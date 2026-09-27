import { browser } from "wxt/browser";
import { sanitize, matches, validSite } from "../lib/policy";
import * as db from "../lib/db";
import { foregroundDelta, shouldRetain } from "../lib/attention";
import type { Policy, State, Ticket, Visit } from "../lib/types";
const host = "com.serein.context";
let chain: Promise<unknown> = Promise.resolve();
const serial = <T>(fn: () => Promise<T>): Promise<T> => {
  const p = chain.then(fn, fn);
  chain = p.catch(() => {});
  return p;
};
async function native(
  s: State,
  op: string,
  payload: unknown,
  epoch = s.policy.capture_epoch,
) {
  return browser.runtime.sendNativeMessage(host, {
    protocol: 1,
    request_id: crypto.randomUUID(),
    source_id: s.source_id,
    op,
    capture_epoch: epoch,
    payload,
  });
}
async function flush() {
  let s = await db.getState();
  if (!s.paired) return;
  try {
    for (const c of (await db.controls()).sort(
      (a, b) => a.epoch - b.epoch || (a.op === "policy.update" ? -1 : 1),
    )) {
      const r = await native(s, c.op, c.payload, c.epoch);
      if (r.status === "error")
        throw new Error(r.error?.remedy || "Policy update failed");
      await db.removeControl(c.id);
    }
    const items = await db.pending();
    const batch = [];
    for (const e of items.slice(0, 32)) {
      if (
        new TextEncoder().encode(JSON.stringify([...batch, e])).length > 64000
      )
        break;
      batch.push(e);
    }
    if (batch.length) {
      const r = await native(s, "ingest", { events: batch });
      if (r.status === "error")
        throw new Error(r.error?.remedy || "Save failed");
      await db.removeEvents([
        ...(r.acknowledged_ids || []),
        ...(r.duplicate_ids || []),
        ...(r.rejected || []).map((x: { id: string }) => x.id),
      ]);
      s.lastStatus = r;
    }
    s.lastError = undefined;
    s.retry = 0;
    await db.saveState(s);
    if ((await db.pending()).length)
      await browser.alarms.create("retry", { when: Date.now() + 1000 });
  } catch {
    s.lastError = "Finish local setup";
    s.retry++;
    await db.saveState(s);
    if (s.retry <= 4)
      await browser.alarms.create("retry", {
        when:
          Date.now() +
          [1000, 5000, 30000, 120000][s.retry - 1] *
            (0.9 + Math.random() * 0.2),
      });
  }
}
async function finalize(v: Visit) {
  const s = await db.getState();
  if (
    v.epoch !== s.policy.capture_epoch ||
    s.policy.paused ||
    !s.policy.consent
  )
    return;
  const elapsed = foregroundDelta(v.lastTick, Date.now(), true);
  v.event.foreground_seconds = Math.min(
    3600,
    Math.floor(v.event.foreground_seconds + elapsed),
  );
  if (shouldRetain(v.event.kind, v.event.foreground_seconds))
    await db.enqueue(v.event);
}
async function observe(checkpoint = false, navigatedTab?: number) {
  let s = await db.getState();
  if (s.pauseUntil && Date.now() >= s.pauseUntil) {
    s.pauseUntil = undefined;
    await db.saveState(s);
    await changePolicy({ ...s.policy, paused: false });
    s = await db.getState();
  }
  const old = await db.visit();
  const focused = await browser.windows.getLastFocused();
  const idle = await browser.idle.queryState(60);
  const tabs =
    focused.focused && idle === "active"
      ? await browser.tabs.query({ active: true, windowId: focused.id })
      : [];
  const tab = tabs[0];
  const meta = tab ? sanitize(tab, s.policy) : null;
  const identity = meta
    ? `${tab.id}|${meta.site_key}|${meta.search_query || ""}`
    : "";
  if (old && meta && old.identity === identity && navigatedTab !== old.tab) {
    old.event.title = meta.title;
    old.event.foreground_seconds += foregroundDelta(
      old.lastTick,
      Date.now(),
      true,
    );
    old.lastTick = Date.now();
    if (
      checkpoint &&
      old.event.kind === "visit" &&
      shouldRetain(old.event.kind, old.event.foreground_seconds)
    ) {
      await db.enqueue({
        ...old.event,
        foreground_seconds: Math.floor(old.event.foreground_seconds),
      });
      old.event.event_id = crypto.randomUUID();
      old.event.foreground_seconds = 0;
    }
    await db.setVisit(old);
  } else {
    if (old) await finalize(old);
    await db.setVisit();
    if (meta && tab.id !== undefined) {
      const site_epoch = Math.max(
        0,
        ...Object.entries(s.siteEpochs)
          .filter(([site]) => matches(meta.site_key, site))
          .map(([, epoch]) => epoch),
      );
      await db.setVisit({
        tab: tab.id,
        window: tab.windowId,
        identity,
        lastTick: Date.now(),
        epoch: s.policy.capture_epoch,
        event: {
          ...meta,
          event_id: crypto.randomUUID(),
          visit_id: crypto.randomUUID(),
          site_epoch,
          observed_at: new Date().toISOString(),
          foreground_seconds: 0,
        },
      });
      if (meta.kind === "search") {
        const current = await db.visit();
        if (current) await db.enqueue(current.event);
      }
    }
  }
  if (await db.visit())
    await browser.alarms.create("attention", { delayInMinutes: 1 });
  else await browser.alarms.clear("attention");
  await flush();
}
async function changePolicy(policy: Policy) {
  const s = await db.getState();
  policy.capture_epoch = s.policy.capture_epoch + 1;
  s.policy = policy;
  for (const site of [...policy.excluded_sites, ...policy.selected_sites])
    s.siteEpochs[site] = policy.capture_epoch;
  await db.privacyTransition(s, {
    id: crypto.randomUUID(),
    op: "policy.update",
    epoch: policy.capture_epoch,
    payload: policy,
  });
  await flush();
}
export default defineBackground(() => {
  browser.runtime.onStartup.addListener(() => {
    void serial(async () => {
      await db.setVisit();
      const s = await db.getState();
      if (s.pauseUntil)
        await browser.alarms.create("resume", {
          when: Math.max(Date.now() + 1000, s.pauseUntil),
        });
    });
  });
  browser.tabs.onUpdated.addListener((tabId, info) => {
    if (info.url || info.title || info.status === "complete")
      void serial(() => observe(false, info.url ? tabId : undefined));
  });
  browser.tabs.onActivated.addListener(() => void serial(() => observe()));
  browser.tabs.onRemoved.addListener(() => void serial(() => observe()));
  browser.windows.onFocusChanged.addListener(
    () => void serial(() => observe()),
  );
  browser.idle.onStateChanged.addListener(() => void serial(() => observe()));
  browser.alarms.onAlarm.addListener(
    (a) =>
      void serial(async () => {
        if (a.name === "retry") await flush();
        else await observe(a.name === "attention");
      }),
  );
  browser.runtime.onMessage.addListener((message, sender) => {
    if (
      sender.id !== browser.runtime.id ||
      !sender.url?.startsWith(browser.runtime.getURL("/"))
    )
      return undefined;
    return serial(async () => {
      const s = await db.getState();
      switch (message.type) {
        case "state":
          await flush();
          return {
            state: await db.getState(),
            queued: (await db.pending()).length,
            controls: (await db.controls()).length,
          };
        case "policy":
          await changePolicy({ ...s.policy, ...message.patch });
          return { ok: true };
        case "pause":
          if (message.minutes) {
            s.pauseUntil = Date.now() + message.minutes * 60000;
            await db.saveState(s);
            await browser.alarms.create("resume", { when: s.pauseUntil });
          } else {
            s.pauseUntil = undefined;
            await db.saveState(s);
          }
          await changePolicy({ ...s.policy, paused: message.paused });
          return { ok: true };
        case "exclude": {
          if (!validSite(message.site)) throw new Error("Invalid site");
          const p = {
            ...s.policy,
            excluded_sites: [
              ...new Set([...s.policy.excluded_sites, message.site]),
            ],
          };
          p.capture_epoch++;
          s.policy = p;
          s.siteEpochs[message.site] = p.capture_epoch;
          await db.privacyTransition(
            s,
            {
              id: crypto.randomUUID(),
              op: "policy.update",
              epoch: p.capture_epoch,
              payload: p,
            },
            message.site,
          );
          if (message.forget)
            await db.privacyTransition(
              s,
              {
                id: crypto.randomUUID(),
                op: "forget",
                epoch: p.capture_epoch,
                payload: {
                  site: message.site,
                  atom_id: null,
                  site_epoch: p.capture_epoch,
                },
              },
              message.site,
            );
          await flush();
          return { ok: true };
        }
        case "include":
          s.siteEpochs[message.site] = s.policy.capture_epoch + 1;
          await db.saveState(s);
          await changePolicy({
            ...s.policy,
            excluded_sites: s.policy.excluded_sites.filter(
              (x) => !matches(message.site, x),
            ),
            selected_sites: [
              ...new Set([...s.policy.selected_sites, message.site]),
            ],
          });
          return { ok: true };
        case "ticket": {
          const t: Ticket = {
            protocol: 1,
            source_id: s.source_id,
            extension_id: browser.runtime.id,
            browser:
              import.meta.env.BROWSER === "firefox" ? "firefox" : "chrome",
            nonce: crypto.randomUUID() + crypto.randomUUID(),
            expires_at: Math.floor(Date.now() / 1000) + 900,
            adapters: message.adapters,
            install_skills: message.install_skills === true,
            skill_repository: __SEREIN_SKILL_REPOSITORY__,
            label: message.label || "Personal",
            consent: true,
          };
          s.ticket = t;
          await db.saveState(s);
          return t;
        }
        case "verify": {
          if (!s.ticket) throw new Error("Create setup instructions first");
          const r = await native(s, "hello", { nonce: s.ticket.nonce });
          if (r.status === "error") throw new Error(r.error.remedy);
          s.paired = true;
          s.lastStatus = r;
          s.lastError = undefined;
          s.policy.capture_epoch = Math.max(
            s.policy.capture_epoch,
            r.policy.capture_epoch,
          );
          await db.saveState(s);
          await changePolicy({ ...s.policy });
          return r;
        }
        case "host": {
          if (message.op === "forget" && message.payload?.atom_id) {
            await changePolicy({ ...s.policy });
            s.policy = (await db.getState()).policy;
          }
          if (!s.paired) throw new Error("Finish local setup");
          await flush();
          if ((await db.controls()).length)
            throw new Error(
              "Privacy changes are pending. Restore local connection first.",
            );
          const r = await native(s, message.op, message.payload || {});
          if (r.status === "error") throw new Error(r.error.remedy);
          return r;
        }
        default:
          throw new Error("Unknown Serein action");
      }
    }).catch((e) => ({ error: String(e.message || e) }));
  });
});

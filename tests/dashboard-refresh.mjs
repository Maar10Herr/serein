import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFile } from "node:fs/promises";
import path from "node:path";

const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function nativeRequests(requestLogPath) {
  const log = await readFile(requestLogPath, "utf8").catch(() => "");
  return log.trim()
    ? log.trim().split("\n").map((line) => JSON.parse(line))
    : [];
}

/**
 * Exercise the shipped dashboard inside a browser harness that has already
 * installed a fresh extension build and connected its real native helper.
 * The only interception is in this disposable extension page: it can delay or
 * rewind a response after the real runtime/native request has completed.
 */
export async function runDashboardRefreshTests({
  page,
  requestLogPath,
  root,
  nativeEnv,
  runtimeDir,
}) {
  const extensionUrl = new URL(page.url());
  const extensionOrigin = `${extensionUrl.protocol}//${extensionUrl.host}`;
  const dashboardUrl = `${extensionOrigin}/dashboard.html#context`;
  const popupUrl = `${extensionOrigin}/popup.html`;

  await page.addInitScript(() => {
    const probe = {
      installed: false,
      requests: [],
      mutationRequests: [],
      results: [],
      held: [],
      heldMutations: [],
      holdDashboardResponses: 0,
      holdNextPrivacyMutation: false,
      rewindNextDashboard: false,
      failNext: null,
      visibility: "visible",
      documentInstance: crypto.randomUUID(),
      releaseNext() {
        const response = this.held.shift();
        if (!response) return false;
        response.resolve(response.value);
        return true;
      },
      releaseMutation() {
        const mutation = this.heldMutations.shift();
        if (!mutation) return false;
        mutation.release();
        return true;
      },
    };
    Object.defineProperty(window, "__sereinRefreshProbe", {
      configurable: false,
      value: probe,
    });
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      get: () => probe.visibility,
    });

    const install = () => {
      const runtimes = [globalThis.chrome?.runtime, globalThis.browser?.runtime]
        .filter(Boolean)
        .filter((runtime, index, all) => all.indexOf(runtime) === index);
      let patched = 0;
      for (const runtime of runtimes) {
        if (runtime.sendMessage?.__sereinRefreshProbe === probe) {
          patched += 1;
          continue;
        }
        const originalSendMessage = runtime.sendMessage.bind(runtime);
        const wrappedSendMessage = (message, ...args) => {
          const privacyMutation =
            (message?.type === "host" && message?.op === "forget") ||
            ["policy", "exclude", "include", "pause"].includes(
              message?.type,
            );
          if (privacyMutation) {
            probe.mutationRequests.push({
              type: message.type,
              op: message.op || "",
              at: performance.now(),
            });
          }
          const tracked =
            message?.type === "state" ||
            (message?.type === "host" && message?.op === "dashboard");
          if (tracked) {
            probe.requests.push({
              type: message.type,
              op: message.op || "",
              at: performance.now(),
            });
          }

          if (
            probe.failNext &&
            message?.type === probe.failNext.type &&
            (probe.failNext.op === undefined ||
              message?.op === probe.failNext.op)
          ) {
            const error = probe.failNext.error;
            probe.failNext = null;
            return Promise.resolve({ error });
          }

          if (
            probe.holdNextPrivacyMutation &&
            privacyMutation
          ) {
            probe.holdNextPrivacyMutation = false;
            return new Promise((resolve, reject) => {
              probe.heldMutations.push({
                release() {
                  try {
                    Promise.resolve(originalSendMessage(message, ...args)).then(
                      resolve,
                      reject,
                    );
                  } catch (error) {
                    reject(error);
                  }
                },
              });
            });
          }

          const response = originalSendMessage(message, ...args);
          if (message?.type !== "host" || message?.op !== "dashboard")
            return response;

          return Promise.resolve(response).then((actual) => {
            let value = actual;
            if (
              probe.rewindNextDashboard &&
              typeof actual?.evidence_generation === "number" &&
              typeof actual?.privacy_generation === "number"
            ) {
              probe.rewindNextDashboard = false;
              value = {
                ...actual,
                cards: [],
                memories: [],
                evidence_generation: actual.evidence_generation - 1,
              };
            }
            probe.results.push({
              evidence_generation: value?.evidence_generation,
              privacy_generation: value?.privacy_generation,
              texts: Array.isArray(value?.cards)
                ? value.cards.map((card) => card.text)
                : [],
            });
            if (probe.holdDashboardResponses > 0) {
              probe.holdDashboardResponses -= 1;
              return new Promise((resolve) =>
                probe.held.push({ value, resolve }),
              );
            }
            return value;
          });
        };
        Object.defineProperty(wrappedSendMessage, "__sereinRefreshProbe", {
          value: probe,
        });
        try {
          Object.defineProperty(runtime, "sendMessage", {
            configurable: true,
            writable: true,
            value: wrappedSendMessage,
          });
        } catch {
          runtime.sendMessage = wrappedSendMessage;
        }
        if (runtime.sendMessage === wrappedSendMessage) patched += 1;
      }
      probe.installed = patched > 0;
      if (probe.installed && retryTimer) clearInterval(retryTimer);
    };
    let retryTimer;
    install();
    retryTimer = setInterval(install, 10);
    setTimeout(() => clearInterval(retryTimer), 5000);
  });

  let auxiliary;
  try {
    await page.goto(dashboardUrl);
    // The browser test often arrives on this exact route already. Force a
    // document navigation so Playwright runs the probe installed above.
    await page.reload();
    await page.waitForFunction(
      () => !document.querySelector(".topbar select")?.disabled,
      undefined,
      { timeout: 15000 },
    );
    await page.locator(".topbar select").first().selectOption("en");
    await page.getByRole("button", { name: "Refresh", exact: true }).waitFor();
    await page.waitForFunction(
      () => window.__sereinRefreshProbe?.installed,
      undefined,
      { timeout: 10000 },
    );
    await page.waitForFunction(() =>
      window.__sereinRefreshProbe.requests.some(
        (request) => request.type === "host" && request.op === "dashboard",
      ),
    );
    await page.waitForFunction(
      () =>
        ![...document.querySelectorAll('[role="status"]')].some((node) =>
          node.textContent?.trim().startsWith("Updating context"),
        ),
      undefined,
      { timeout: 15000 },
    );
    await page.getByText(
      "This view shows recent activity and selected research evidence, not every saved observation.",
      { exact: true },
    ).waitFor();

    const dashboardCount = () =>
      page.evaluate(
        () =>
          window.__sereinRefreshProbe.requests.filter(
            (request) => request.type === "host" && request.op === "dashboard",
          ).length,
      );
    const waitForDashboardCount = async (count) =>
      page.waitForFunction(
        (expected) =>
          window.__sereinRefreshProbe.requests.filter(
            (request) => request.type === "host" && request.op === "dashboard",
          ).length >= expected,
        count,
        { timeout: 15000 },
      );
    const stateCount = () =>
      page.evaluate(
        () =>
          window.__sereinRefreshProbe.requests.filter(
            (request) => request.type === "state",
          ).length,
      );
    const waitForStateCount = (count) =>
      page.waitForFunction(
        (expected) =>
          window.__sereinRefreshProbe.requests.filter(
            (request) => request.type === "state",
          ).length >= expected,
        count,
        { timeout: 15000 },
      );
    const waitForRefreshIdle = () =>
      page.waitForFunction(
        () =>
          ![...document.querySelectorAll('[role="status"]')].some((node) =>
            node.textContent?.trim().startsWith("Updating context"),
          ),
        undefined,
        { timeout: 15000 },
      );
    const rawActivity = page.locator(".raw-activity details");
    const openRawActivity = async () => {
      await rawActivity.waitFor();
      if (!(await rawActivity.evaluate((element) => element.open)))
        await rawActivity.locator("summary").click();
    };
    const cardFor = (text) =>
      page.locator(".raw-activity .evidence-card").filter({ hasText: text });
    const probe = (script) => page.evaluate(script);

    assert.equal(
      await probe(() => window.__sereinRefreshProbe.installed),
      true,
      "the test-only runtime response wrapper was not installed",
    );

    // A stable connected dashboard must not create a timer-driven query loop.
    const afterInitial = await dashboardCount();
    const initialDocument = await probe(
      () => window.__sereinRefreshProbe.documentInstance,
    );
    await pause(3200);
    assert.equal(
      await dashboardCount(),
      afterInitial,
      "the dashboard issued a periodic refresh while idle",
    );

    // R22: queue a real IndexedDB outbox event and let the extension flush it
    // through its normal state/manual-refresh path and native messaging host.
    const outboxEvent = await page.evaluate(async () => {
      const opened = indexedDB.open("serein", 1);
      const database = await new Promise((resolve, reject) => {
        opened.onsuccess = () => resolve(opened.result);
        opened.onerror = () => reject(opened.error);
      });
      const event = {
        event_id: crypto.randomUUID(),
        visit_id: crypto.randomUUID(),
        site_key: "t09-outbox.example.org",
        site_epoch: 0,
        observed_at: new Date().toISOString(),
        kind: "search",
        title: "T09 outbox event before refresh",
        search_query: "T09 outbox event before refresh",
        foreground_seconds: 30,
      };
      const transaction = database.transaction(["outbox", "state"], "readwrite");
      const completed = new Promise((resolve, reject) => {
        transaction.oncomplete = resolve;
        transaction.onerror = () => reject(transaction.error);
        transaction.onabort = () => reject(transaction.error);
      });
      const stateRequest = transaction.objectStore("state").get("config");
      stateRequest.onsuccess = () => {
        const state = stateRequest.result;
        state.batchSince = Date.now() - 61000;
        transaction.objectStore("state").put(state, "config");
        transaction.objectStore("outbox").put(event);
      };
      await completed;
      database.close();
      return event;
    });
    const documentBeforeRefresh = await probe(
      () => window.__sereinRefreshProbe.documentInstance,
    );
    const beforeManualRefresh = await dashboardCount();
    await page.getByRole("button", { name: "Refresh", exact: true }).click();
    await waitForDashboardCount(beforeManualRefresh + 1);
    await waitForRefreshIdle();
    await openRawActivity();
    await cardFor("T09 outbox event before refresh").waitFor();
    assert.equal(
      await probe(() => window.__sereinRefreshProbe.documentInstance),
      documentBeforeRefresh,
      "manual refresh reloaded the extension page",
    );
    assert.equal(initialDocument, documentBeforeRefresh);
    assert.ok(
      (await nativeRequests(requestLogPath)).some(
        (request) =>
          request.op === "ingest" &&
          request.payload?.events?.some(
            (event) => event.event_id === outboxEvent.event_id,
          ),
      ),
      "the synthetic event did not pass through the real extension outbox flush",
    );

    // An independent extension view corrects the card. The current dashboard
    // learns about it only after its normal refresh, with no reload.
    const baselineDashboard = await nativeRequests(requestLogPath);
    const ingest = baselineDashboard.find(
      (request) =>
        request.op === "ingest" &&
        request.payload?.events?.some(
          (event) => event.event_id === outboxEvent.event_id,
        ),
    );
    assert.ok(ingest, "the outbox event was not acknowledged by the helper");
    const liveDashboard = await page.evaluate(() =>
      (globalThis.browser?.runtime || globalThis.chrome?.runtime).sendMessage({
        type: "host",
        op: "dashboard",
      }),
    );
    assert.ok(
      typeof liveDashboard.evidence_generation === "number" &&
        typeof liveDashboard.privacy_generation === "number",
      "the native dashboard did not return generation fields",
    );
    const correctedCard = liveDashboard.cards.find((card) =>
      card.text.includes("T09 outbox event before refresh"),
    );
    assert.ok(correctedCard, "the refreshed dashboard omitted the new event");
    auxiliary = await page.context().newPage();
    await auxiliary.goto(`${extensionOrigin}/dashboard.html#connections`);
    const correctionText = "T09 externally corrected from another view";
    const externalCorrection = await auxiliary.evaluate(
      ({ atomId, text }) =>
        (globalThis.browser?.runtime || globalThis.chrome?.runtime).sendMessage({
          type: "host",
          op: "feedback",
          payload: {
            atom_id: atomId,
            action: "confirm_constraint",
            text,
          },
        }),
      { atomId: correctedCard.id, text: correctionText },
    );
    assert.ok(!externalCorrection?.error, JSON.stringify(externalCorrection));
    const searchInput = page.getByRole("textbox", {
      name: "Filter this view",
    });
    await searchInput.fill(correctionText);
    await page.getByRole("button", { name: "Confirmed", exact: true }).click();
    const beforeExternalRefresh = await dashboardCount();
    await page.getByRole("button", { name: "Refresh", exact: true }).click();
    await waitForDashboardCount(beforeExternalRefresh + 1);
    await cardFor(correctionText).waitFor();
    await waitForRefreshIdle();
    assert.equal(await searchInput.inputValue(), correctionText);
    assert.equal(
      await page
        .getByRole("button", { name: "Confirmed", exact: true })
        .getAttribute("aria-pressed"),
      "true",
      "manual refresh discarded the selected state filter",
    );
    assert.equal(new URL(page.url()).hash, "#context");
    await searchInput.fill("");
    await page.getByRole("button", { name: "All context", exact: true }).click();
    await auxiliary.close();
    auxiliary = undefined;

    // A response with older backend generations must leave the last safe view
    // in place and report stale state.
    await probe(() => {
      window.__sereinRefreshProbe.rewindNextDashboard = true;
    });
    const beforeStaleResponse = await dashboardCount();
    await page.getByRole("button", { name: "Refresh", exact: true }).click();
    await waitForDashboardCount(beforeStaleResponse + 1);
    await page.getByText(
      "Showing the last available context. Refresh to check for updates.",
      { exact: true },
    ).waitFor();
    await cardFor(correctionText).waitFor();
    const staleResponse = await probe(() =>
      window.__sereinRefreshProbe.results.at(-1),
    );
    assert.ok(
      typeof staleResponse.evidence_generation === "number" &&
        typeof staleResponse.privacy_generation === "number",
      "the native dashboard did not return generation fields for stale-response coverage",
    );

    // Paired focus/visibility and Context-entry signals share one 150 ms load.
    await page
      .locator(".nav button")
      .filter({ hasText: "Connections" })
      .evaluate((button) => button.click());
    await page.waitForFunction(() => location.hash.slice(1) === "connections");
    const beforeBurst = await dashboardCount();
    const beforeBurstState = await stateCount();
    const burstStartedAt = await probe(() => performance.now());
    await page
      .locator(".nav button")
      .filter({ hasText: "Your context" })
      .evaluate((button) => button.click());
    await page.waitForFunction(() => location.hash.slice(1) === "context");
    await page.evaluate(() => {
      window.dispatchEvent(new Event("focus"));
      document.dispatchEvent(new Event("visibilitychange"));
    });
    await waitForStateCount(beforeBurstState + 1);
    const coalescedLoadStartedAt = await probe(
      () =>
        window.__sereinRefreshProbe.requests
          .filter((request) => request.type === "state")
          .at(-1).at,
    );
    assert.ok(
      coalescedLoadStartedAt - burstStartedAt >= 135,
      `focus/visibility refresh started before the 150 ms debounce: ${coalescedLoadStartedAt - burstStartedAt} ms`,
    );
    await waitForDashboardCount(beforeBurst + 1);
    await pause(250);
    assert.equal(await dashboardCount(), beforeBurst + 1);
    await waitForRefreshIdle();

    // Hidden documents keep one dirty refresh until visible again.
    const beforeHidden = await dashboardCount();
    await probe(() => {
      window.__sereinRefreshProbe.visibility = "hidden";
      window.dispatchEvent(new Event("focus"));
      document.dispatchEvent(new Event("visibilitychange"));
    });
    await pause(250);
    assert.equal(await dashboardCount(), beforeHidden);
    await probe(() => {
      window.__sereinRefreshProbe.visibility = "visible";
      document.dispatchEvent(new Event("visibilitychange"));
    });
    await waitForDashboardCount(beforeHidden + 1);
    await pause(250);
    assert.equal(await dashboardCount(), beforeHidden + 1);
    await waitForRefreshIdle();

    // Multiple signals during one delayed request produce a single trailing
    // load after the in-flight response settles.
    await probe(() => {
      window.__sereinRefreshProbe.holdDashboardResponses = 1;
    });
    const beforeInflight = await dashboardCount();
    await page.getByRole("button", { name: "Refresh", exact: true }).click();
    await page.waitForFunction(
      () => window.__sereinRefreshProbe.held.length === 1,
      undefined,
      { timeout: 15000 },
    );
    await page.evaluate(() => {
      window.dispatchEvent(new Event("focus"));
      document.dispatchEvent(new Event("visibilitychange"));
      window.dispatchEvent(new Event("focus"));
      document.dispatchEvent(new Event("visibilitychange"));
    });
    await pause(250);
    assert.equal(
      await dashboardCount(),
      beforeInflight + 1,
      "a second load started while the first response was held",
    );
    await probe(() => window.__sereinRefreshProbe.releaseNext());
    await waitForDashboardCount(beforeInflight + 2);
    await waitForRefreshIdle();
    assert.equal(
      await dashboardCount(),
      beforeInflight + 2,
      "in-flight refresh signals did not coalesce to one trailing load",
    );

    // A privacy mutation advances the UI revision before its request. Release
    // an older successful read after Forget clears the card; it must not
    // restore that card while the authoritative trailing read is held.
    const beforePrivacyRace = await dashboardCount();
    await probe(() => {
      window.__sereinRefreshProbe.holdDashboardResponses = 2;
    });
    await page.getByRole("button", { name: "Refresh", exact: true }).click();
    await page.waitForFunction(
      () => window.__sereinRefreshProbe.held.length === 1,
      undefined,
      { timeout: 15000 },
    );
    await openRawActivity();
    const privacyTarget = cardFor(correctionText);
    await privacyTarget.getByRole("button", { name: "Forget", exact: true }).click();
    await page
      .getByRole("dialog", { name: "Forget this observation?", exact: true })
      .getByRole("button", { name: "Forget observation", exact: true })
      .click();
    await page
      .getByRole("dialog", { name: "Forget this observation?", exact: true })
      .waitFor({ state: "detached" });
    assert.equal(await cardFor(correctionText).count(), 0);
    await probe(() => window.__sereinRefreshProbe.releaseNext());
    await page.waitForFunction(
      () => window.__sereinRefreshProbe.held.length === 1,
      undefined,
      { timeout: 15000 },
    );
    assert.equal(
      await cardFor(correctionText).count(),
      0,
      "the superseded successful response restored forgotten context",
    );
    await probe(() => window.__sereinRefreshProbe.releaseNext());
    await waitForDashboardCount(beforePrivacyRace + 2);
    await waitForRefreshIdle();
    assert.equal(await cardFor(correctionText).count(), 0);

    // R21: 61 real outbox observations put the retained telescope item beyond
    // the 60-card view window. The local empty result stays scoped to this view,
    // while the documented topic-bearing native question still finds it.
    const windowEvents = await page.evaluate(async () => {
      const opened = indexedDB.open("serein", 1);
      const database = await new Promise((resolve, reject) => {
        opened.onsuccess = () => resolve(opened.result);
        opened.onerror = () => reject(opened.error);
      });
      const now = Date.now();
      const telescope = {
        event_id: crypto.randomUUID(),
        visit_id: crypto.randomUUID(),
        site_key: "telescope-r21.example.org",
        site_epoch: 0,
        observed_at: new Date(now - 24 * 60 * 60 * 1000).toISOString(),
        kind: "search",
        title: "Telescope mirror collimation guide",
        search_query: "Telescope mirror collimation guide",
        foreground_seconds: 30,
      };
      const recent = Array.from({ length: 60 }, (_, index) => ({
        event_id: crypto.randomUUID(),
        visit_id: crypto.randomUUID(),
        site_key: `t09-window-${index}.example.org`,
        site_epoch: 0,
        observed_at: new Date(now - index * 1000).toISOString(),
        kind: "search",
        title: `T09 display-window observation ${index}`,
        search_query: `T09 display-window observation ${index}`,
        foreground_seconds: 30,
      }));
      const transaction = database.transaction(["outbox", "state"], "readwrite");
      const completed = new Promise((resolve, reject) => {
        transaction.oncomplete = resolve;
        transaction.onerror = () => reject(transaction.error);
        transaction.onabort = () => reject(transaction.error);
      });
      const stateRequest = transaction.objectStore("state").get("config");
      stateRequest.onsuccess = () => {
        const state = stateRequest.result;
        state.batchSince = Date.now() - 61000;
        transaction.objectStore("state").put(state, "config");
        transaction.objectStore("outbox").put(telescope);
        for (const event of recent)
          transaction.objectStore("outbox").put(event);
      };
      await completed;
      database.close();
      return { telescope, recent };
    });
    const outboxCount = () =>
      page.evaluate(async () => {
        const opened = indexedDB.open("serein", 1);
        const database = await new Promise((resolve, reject) => {
          opened.onsuccess = () => resolve(opened.result);
          opened.onerror = () => reject(opened.error);
        });
        const transaction = database.transaction("outbox", "readonly");
        const completed = new Promise((resolve, reject) => {
          transaction.oncomplete = resolve;
          transaction.onerror = () => reject(transaction.error);
          transaction.onabort = () => reject(transaction.error);
        });
        const request = transaction.objectStore("outbox").count();
        const count = await new Promise((resolve, reject) => {
          request.onsuccess = () => resolve(request.result);
          request.onerror = () => reject(request.error);
        });
        await completed;
        database.close();
        return count;
      });
    for (let attempt = 0; attempt < 5 && (await outboxCount()) > 0; attempt++) {
      const beforeWindowRefresh = await dashboardCount();
      await page.getByRole("button", { name: "Refresh", exact: true }).click();
      await waitForDashboardCount(beforeWindowRefresh + 1);
      await waitForRefreshIdle();
    }
    assert.equal(await outboxCount(), 0, "manual refresh did not drain the real outbox");
    const windowDashboard = await page.evaluate(() =>
      (globalThis.browser?.runtime || globalThis.chrome?.runtime).sendMessage({
        type: "host",
        op: "dashboard",
      }),
    );
    assert.ok(
      windowDashboard.cards.length <= 60,
      `dashboard exceeded its 60-card window: ${windowDashboard.cards.length}`,
    );
    assert.ok(
      !windowDashboard.cards.some((card) =>
        card.text.includes("Telescope mirror collimation guide"),
      ),
      "the older retained telescope record unexpectedly appeared in the recent view",
    );
    assert.ok(
      !(windowDashboard.memories || []).some(
        (memory) =>
          memory.label.includes("Telescope mirror collimation guide") ||
          memory.items.some((item) =>
            item.text.includes("Telescope mirror collimation guide"),
          ),
      ),
      "the older retained telescope record unexpectedly appeared in a research group",
    );
    assert.ok(
      windowDashboard.atoms > 60,
      `the fixture should exceed the 60-card view window; got ${windowDashboard.atoms} saved observations`,
    );
    const windowEventIds = new Set(
      [windowEvents.telescope, ...windowEvents.recent].map(
        (event) => event.event_id,
      ),
    );
    const windowIngestRequests = (await nativeRequests(requestLogPath)).filter(
      (request) =>
        request.op === "ingest" &&
        request.payload?.events?.some((event) =>
          windowEventIds.has(event.event_id),
        ),
    );
    assert.ok(
      windowIngestRequests.length >= 2,
      "61 queued observations should require multiple native batches",
    );
    for (const request of windowIngestRequests) {
      assert.ok(
        request.payload.events.length <= 32,
        `native ingest batch exceeded 32 events: ${request.payload.events.length}`,
      );
      assert.ok(
        new TextEncoder().encode(JSON.stringify(request.payload.events)).length <=
          64 * 1024,
        "native ingest batch exceeded the 64 KiB wire limit",
      );
    }
    const windowIngestedIds = new Set(
      windowIngestRequests.flatMap((request) =>
        request.payload.events.map((event) => event.event_id),
      ),
    );
    for (const event of [windowEvents.telescope, ...windowEvents.recent])
      assert.ok(
        windowIngestedIds.has(event.event_id),
        `outbox event ${event.event_id} was not acknowledged by the native helper`,
      );
    await openRawActivity();
    const beforeLocalFilter = await dashboardCount();
    await searchInput.fill("Telescope mirror collimation");
    await rawActivity
      .getByText("No matches in the displayed context.", { exact: true })
      .waitFor();
    assert.equal(
      await dashboardCount(),
      beforeLocalFilter,
      "filter keystrokes triggered a native dashboard load",
    );
    assert.equal(
      await page.getByText(
        "This view shows recent activity and selected research evidence, not every saved observation.",
        { exact: true },
      ).count(),
      1,
    );
    await searchInput.fill("");
    const recallRequest = JSON.stringify({
      protocol: 1,
      request_id: crypto.randomUUID(),
      client: "generic",
      vault: "default",
      query: "What did I find about telescope mirror collimation?",
      facets: [],
      scope: ["research"],
      max_bytes: 4096,
      budget_ms: 1500,
    });
    const recall = JSON.parse(
      (runtimeDir
        ? execFileSync(path.join(runtimeDir, "serein"), ["recall"], {
            input: recallRequest,
            env: nativeEnv,
            cwd: runtimeDir,
          })
        : execFileSync(
            "sh",
            [path.join(root, "skills/serein-context/scripts/recall.sh")],
            { input: recallRequest, env: nativeEnv, cwd: root },
          )
      ).toString(),
    );
    assert.ok(
      JSON.stringify(recall).includes("Telescope mirror collimation guide"),
      "the documented topic-bearing native question did not find the retained item",
    );

    // R24: check the user-facing browser selectors in all six supported
    // locales; the catalog unit suite separately verifies key/placeholder
    // parity.
    const localeLabels = [
      ["en", "Filter this view", "Refresh"],
      ["de", "Diese Ansicht filtern", "Aktualisieren"],
      ["nl", "Deze weergave filteren", "Vernieuwen"],
      ["zh-CN", "筛选此视图", "刷新"],
      ["ja", "このビューを絞り込む", "更新"],
      ["es", "Filtrar esta vista", "Actualizar"],
    ];
    const languageSelect = page.locator(".topbar select").first();
    for (const [locale, filterLabel, refreshLabel] of localeLabels) {
      await languageSelect.selectOption(locale);
      await page.waitForFunction(
        (expected) => document.documentElement.lang === expected,
        locale,
      );
      assert.equal(
        await page.getByRole("textbox").getAttribute("aria-label"),
        filterLabel,
        `${locale} filter selector changed or is missing`,
      );
      assert.equal(
        await page.getByRole("button", { name: refreshLabel, exact: true }).count(),
        1,
        `${locale} refresh selector changed or is missing`,
      );
    }
    await languageSelect.selectOption("en");
    await page.waitForFunction(() => document.documentElement.lang === "en");

    // Leaving the dashboard must not send a delayed focus refresh to the helper.
    await waitForRefreshIdle();
    await pause(200);
    const dashboardRequestsBeforeUnmount = (await nativeRequests(requestLogPath))
      .filter((request) => request.op === "dashboard").length;
    await page.evaluate(() => window.dispatchEvent(new Event("focus")));
    await page.goto(popupUrl);
    await pause(250);
    assert.equal(
      (await nativeRequests(requestLogPath)).filter(
        (request) => request.op === "dashboard",
      ).length,
      dashboardRequestsBeforeUnmount,
      "the closed dashboard sent a delayed refresh to the helper",
    );
    await page.goto(dashboardUrl);
    await page.locator(".topbar select").first().selectOption("en");
    await page.getByRole("button", { name: "Refresh", exact: true }).waitFor();
    await page.waitForFunction(() =>
      window.__sereinRefreshProbe.requests.some(
        (request) => request.type === "host" && request.op === "dashboard",
      ),
    );
    await waitForRefreshIdle();

    // Rapid privacy-control clicks must serialize behind the first pending
    // mutation. Hold the first real policy request, click a second control in
    // the same browser task, and verify the disabled controls and mutation
    // guard prevent another request from being sent.
    await page
      .locator(".nav button")
      .filter({ hasText: "Privacy" })
      .click();
    await page.getByRole("heading", { name: "Privacy", exact: true }).waitFor();
    const recallToggle = page.getByRole("checkbox", {
      name: "Allow assistant recall",
      exact: true,
    });
    const selectedSitesToggle = page.getByRole("checkbox", {
      name: "Only selected sites",
      exact: true,
    });
    await probe(() => {
      window.__sereinRefreshProbe.holdNextPrivacyMutation = true;
    });
    const beforeOverlappingMutations = await probe(
      () => window.__sereinRefreshProbe.mutationRequests.length,
    );
    await page.evaluate(() => {
      const recall = document.querySelector(
        'input[type="checkbox"][aria-label="Allow assistant recall"]',
      );
      const selectedSites = document.querySelector(
        'input[type="checkbox"][aria-label="Only selected sites"]',
      );
      if (!(recall instanceof HTMLInputElement) ||
          !(selectedSites instanceof HTMLInputElement))
        throw new Error("privacy control fixture is missing");
      recall.click();
      selectedSites.click();
    });
    await page.waitForFunction(() => {
      const probe = window.__sereinRefreshProbe;
      const controls = [
        ...document.querySelectorAll('.setting input[type="checkbox"]'),
      ];
      return probe.heldMutations.length === 1 &&
        controls.length >= 3 &&
        controls.every((control) => control.disabled);
    });
    const overlappingMutations = await page.evaluate((before) => ({
      requests: window.__sereinRefreshProbe.mutationRequests.slice(before),
      held: window.__sereinRefreshProbe.heldMutations.length,
      recallDisabled: document.querySelector(
        'input[aria-label="Allow assistant recall"]',
      )?.disabled,
      selectedSitesDisabled: document.querySelector(
        'input[aria-label="Only selected sites"]',
      )?.disabled,
    }), beforeOverlappingMutations);
    assert.equal(overlappingMutations.requests.length, 1, overlappingMutations);
    assert.equal(overlappingMutations.requests[0].type, "policy");
    assert.equal(overlappingMutations.held, 1, overlappingMutations);
    assert.equal(overlappingMutations.recallDisabled, true, overlappingMutations);
    assert.equal(
      overlappingMutations.selectedSitesDisabled,
      true,
      overlappingMutations,
    );
    await probe(() => window.__sereinRefreshProbe.releaseMutation());
    await page.waitForFunction(() =>
      !document.querySelector(
        'input[aria-label="Allow assistant recall"]',
      )?.disabled,
    );
    await page
      .locator(".nav button")
      .filter({ hasText: "Your context" })
      .click();
    await page.waitForFunction(() => location.hash.slice(1) === "context");
    await waitForRefreshIdle();

    // A failed local privacy request must keep context withheld. The following
    // manual refresh really reads the populated native dashboard; the wrapper
    // records that response so this checks the privacy gate after an unrelated
    // successful read rather than after a stubbed empty result.
    await openRawActivity();
    const failureTargetText = "T09 display-window observation 59";
    const failureTarget = cardFor(failureTargetText);
    await failureTarget.waitFor();
    await waitForRefreshIdle();
    await pause(200);
    const beforeFailedForget = await dashboardCount();
    await probe(() => {
      window.__sereinRefreshProbe.failNext = {
        type: "host",
        op: "forget",
        error: "Synthetic privacy mutation failure",
      };
    });
    await failureTarget.getByRole("button", { name: "Forget", exact: true }).click();
    const forgetDialog = page.getByRole("dialog", {
      name: "Forget this observation?",
      exact: true,
    });
    await forgetDialog
      .getByRole("button", { name: "Forget observation", exact: true })
      .click();
    await page
      .getByRole("alert")
      .filter({ hasText: "Synthetic privacy mutation failure" })
      .waitFor();
    await page
      .getByText("This privacy change is not confirmed.", { exact: false })
      .waitFor();
    assert.equal(await cardFor(failureTargetText).count(), 0);
    await waitForDashboardCount(beforeFailedForget + 1);
    await waitForRefreshIdle();
    assert.equal(await cardFor(failureTargetText).count(), 0);
    const beforeUnrelatedRefresh = await dashboardCount();
    await page.getByRole("button", { name: "Refresh", exact: true }).click();
    await waitForDashboardCount(beforeUnrelatedRefresh + 1);
    await waitForRefreshIdle();
    assert.equal(
      await page
        .getByText("This privacy change is not confirmed.", { exact: false })
        .count(),
      1,
    );
    assert.equal(await cardFor(failureTargetText).count(), 0);
    const realPostFailureRead = await probe(() =>
      window.__sereinRefreshProbe.results.at(-1),
    );
    assert.ok(
      realPostFailureRead.texts.includes(failureTargetText),
      "the unrelated refresh did not return real populated native data",
    );
  } finally {
    await page
      .evaluate(() => {
        const probe = window.__sereinRefreshProbe;
        if (!probe) return;
        probe.holdDashboardResponses = 0;
        while (probe.held.length) probe.releaseNext();
      })
      .catch(() => {});
    await auxiliary?.close().catch(() => {});
  }
}

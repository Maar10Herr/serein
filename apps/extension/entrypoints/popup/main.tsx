import { render } from "preact";
import { useEffect, useState } from "preact/hooks";
import { browser } from "wxt/browser";
import { call, openDashboard } from "../../lib/client";
import { Icon } from "../../lib/Icon";
import { Modal } from "../../lib/Modal";
import { matches, validSite } from "../../lib/policy";
import { t, type Locale, type MessageKey } from "../../lib/locales";
import type { State } from "../../lib/types";
import "../../lib/styles.css";
function App() {
  const [locale, setLocale] = useState<Locale>("en");
  const tr = (
    key: MessageKey,
    values?: Readonly<Record<string, string | number>>,
  ) => t(locale, key, values);
  const [s, setS] = useState<State>();
  const [site, setSite] = useState("");
  const [dialog, setDialog] = useState("");
  const [error, setError] = useState("");
  const [pending, setPending] = useState(0);
  const [busy, setBusy] = useState(false);
  async function reload() {
    const r = await call({ type: "state" });
    setS(r.state);
    setPending(r.controls);
  }
  useEffect(() => {
    reload().catch((e) => setError(e.message));
    browser.tabs.query({ active: true, currentWindow: true }).then(([tab]) => {
      try {
        const u = new URL(tab.url || "");
        if (
          !tab.incognito &&
          ["https:", "http:"].includes(u.protocol) &&
          validSite(u.hostname)
        )
          setSite(u.hostname);
      } catch {}
    });
    browser.storage.local.get(["theme", "themeMode", "locale"]).then((x) => {
      if (typeof x.locale === "string") setLocale(x.locale as Locale);
      if ((x.themeMode === "manual" && (x.theme === "light" || x.theme === "dark")) ||
          (x.themeMode === undefined && x.theme === "dark"))
        document.documentElement.dataset.theme = x.theme;
    });
  }, []);
  useEffect(() => {
    document.documentElement.lang = locale;
  }, [locale]);
  async function action(msg: any) {
    setBusy(true);
    try {
      await call(msg);
      await reload();
      setDialog("");
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  const excluded = s?.policy.excluded_sites.some((r) => matches(site, r));
  return (
    <div class="popup">
      <header class="row between">
        <div class="brand row">
          <Icon name="mark" size={29} />
          Serein
        </div>
        <span class="badge">
          <span class="dot" />
          {s?.policy.paused
            ? tr("status.paused")
            : s?.paired && s.policy.consent
              ? tr("status.saving")
              : tr("status.notConnected")}
        </span>
      </header>
      {!s?.paired && (
        <div class="notice setup-note">
          {tr("app.tagline")}
          <button
            class="primary"
            style={{ width: "100%", marginTop: 12 }}
            onClick={() => openDashboard("connections")}
          >
            {tr("popup.finishLocalSetup")} <Icon name="arrow" size={16} />
          </button>
        </div>
      )}
      <p class="site">{site || tr("popup.pageUnavailable")}</p>
      <div class="controls">
        <button onClick={() => openDashboard()}>
          <Icon name="settings" />
          {tr("popup.actions.settings")}
        </button>
        <button
          disabled={!site || busy}
          onClick={() =>
            excluded ? action({ type: "include", site }) : setDialog("exclude")
          }
        >
          <Icon name={excluded ? "include" : "exclude"} />
          {excluded
            ? tr("popup.actions.includeSite")
            : tr("popup.actions.excludeSite")}
        </button>
        <button
          disabled={busy}
          onClick={() =>
            s?.policy.paused
              ? action({ type: "pause", paused: false })
              : setDialog("pause")
          }
        >
          <Icon name={s?.policy.paused ? "resume" : "private"} />
          {s?.policy.paused
            ? tr("popup.actions.resumeSaving")
            : tr("popup.actions.privateSession")}
        </button>
      </div>
      {pending > 0 && (
        <div class="notice" style={{ marginTop: 12 }}>
          {tr("popup.pendingPrivacy")}
        </div>
      )}
      {error && (
        <p role="alert" class="error">
          {error}
        </p>
      )}
      <div class="summary">
        <div class="row" style={{ gap: 7 }}>
          <Icon name="device" size={15} />
          <p>
            {tr("popup.savedLocally")} · {tr("popup.contextOnDemand")}
          </p>
        </div>
        <div class="status-link">
          <span class="muted">
            {s?.paired
              ? tr("popup.observationsOnDevice", {
                  count: s.lastStatus?.atoms || 0,
                })
              : tr("popup.localHelperNotConnected")}
          </span>
          <button
            class="ghost"
            style={{ padding: 0, fontSize: 11 }}
            onClick={() => openDashboard("connections")}
          >
            {s?.paired ? tr("connections.heading") : tr("popup.connect")}{" "}
            <Icon name="arrow" size={13} />
          </button>
        </div>
      </div>
      {dialog === "exclude" && (
        <Modal
          title={tr("exclude.title", { domain: site })}
          onClose={() => setDialog("")}
        >
          <p>{tr("popup.siteSubdomains")}</p>
          <button
            class="choice"
            disabled={busy}
            onClick={() => action({ type: "exclude", site, forget: false })}
          >
            {tr("exclude.futureActivity")}
            <small>{tr("popup.keepSavedContext")}</small>
          </button>
          <button
            class="choice danger"
            disabled={busy}
            onClick={() => action({ type: "exclude", site, forget: true })}
          >
            {tr("exclude.andForget")}
            <small>{tr("popup.deleteSavedEvidence")}</small>
          </button>
        </Modal>
      )}
      {dialog === "pause" && (
        <Modal title={tr("popup.privacyTitle")} onClose={() => setDialog("")}>
          <p>{tr("popup.pauseExplanation")}</p>
          <button
            class="choice"
            onClick={() => action({ type: "pause", paused: true })}
          >
            {tr("popup.pauseUntilResume")}
            <small>{tr("popup.pausePersists")}</small>
          </button>
          <button
            class="choice"
            onClick={() => action({ type: "pause", paused: true, minutes: 30 })}
          >
            {tr("popup.pause30m")}
            <small>{tr("popup.pauseAutoResume")}</small>
          </button>
        </Modal>
      )}
    </div>
  );
}
render(<App />, document.getElementById("app")!);

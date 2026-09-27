import { render } from "preact";
import { useEffect, useState } from "preact/hooks";
import type { ComponentChildren } from "preact";
import { browser } from "wxt/browser";
import { call, host } from "../../lib/client";
import { Icon } from "../../lib/Icon";
import { Modal } from "../../lib/Modal";
import { t, type Locale, type MessageKey } from "../../lib/locales";
import { validSite } from "../../lib/policy";
import type { State } from "../../lib/types";
import "../../lib/styles.css";
const assistants = [
  ["claude-code", "connections.claudeCode", "C"],
  ["codex", "connections.codex", "Cx"],
  ["opencode", "connections.openCode", "Oc"],
  ["hermes", "connections.hermes", "H"],
  ["openclaw", "connections.openClaw", "Cl"],
  ["generic", "connections.genericExecutor", ">_"],
] as const;
const skillRepository = __SEREIN_SKILL_REPOSITORY__;
const skillAgentIds: Record<string, string> = {
  "claude-code": "claude-code",
  codex: "codex",
  opencode: "opencode",
  hermes: "hermes-agent",
  openclaw: "openclaw",
};
const nav = [
  ["context", "context"],
  ["connections", "link"],
  ["privacy", "exclude"],
  ["storage", "storage"],
  ["about", "why"],
];
const correctionLabels: Record<string, MessageKey> = {
  not_about_me: "correction.notAboutMe",
  wrong_topic: "correction.wrongTopic",
  temporary_research: "correction.temporaryResearch",
  confirm_constraint: "correction.confirmConstraint",
  do_not_use: "correction.doNotUse",
};
function Toggle({
  checked,
  onChange,
  label,
}: {
  checked: boolean;
  onChange: () => void;
  label: string;
}) {
  return (
    <label class="toggle">
      <input
        type="checkbox"
        checked={checked}
        onChange={onChange}
        aria-label={label}
      />
      <span />
    </label>
  );
}
function Setting({
  title,
  description,
  children,
}: {
  title: string;
  description: string;
  children: ComponentChildren;
}) {
  return (
    <div class="setting">
      <div>
        <h3>{title}</h3>
        <p>{description}</p>
      </div>
      {children}
    </div>
  );
}
function App() {
  const [section, setSection] = useState(location.hash.slice(1) || "context");
  const [locale, setLocale] = useState<Locale>("en");
  const [theme, setTheme] = useState<"system" | "light" | "dark">("system");
  const [prefsLoaded, setPrefsLoaded] = useState(false);
  const [s, setS] = useState<State>();
  const [data, setData] = useState<any>({ cards: [] });
  const [queue, setQueue] = useState(0);
  const [pending, setPending] = useState(0);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [busy, setBusy] = useState(false);
  const [filter, setFilter] = useState("all");
  const [search, setSearch] = useState("");
  const [selected, setSelected] = useState(["codex"]);
  const [consent, setConsent] = useState(false);
  const [disclosure, setDisclosure] = useState(false);
  const [selectedOnly, setSelectedOnly] = useState(false);
  const [setupText, setSetupText] = useState("");
  const [site, setSite] = useState("");
  const [modal, setModal] = useState<{ kind: string; card?: any } | null>(null);
  const [constraint, setConstraint] = useState("");
  const [receipt, setReceipt] = useState<any[]>([]);
  const tr = (
    key: MessageKey,
    values?: Readonly<Record<string, string | number>>,
  ) => t(locale, key, values);
  const skillTargets = selected
    .map((id) => skillAgentIds[id])
    .filter((id): id is string => Boolean(id));
  const skillInstallCommand =
    skillRepository && skillTargets.length
      ? `npx --yes skills add ${skillRepository} --skill serein-context ${skillTargets.map((id) => `--agent ${id}`).join(" ")} --global --yes --copy`
      : "";
  async function load() {
    const r = await call({ type: "state" });
    setS(r.state);
    if (r.state.lastError?.startsWith("Pairing ticket expired"))
      setError(r.state.lastError);
    setQueue(r.queued);
    setPending(r.controls);
    if (r.state.paired) {
      try {
        setData(await host("dashboard"));
      } catch (e) {
        setError((e as Error).message);
      }
    }
  }
  useEffect(() => {
    load().catch((e) => setError(e.message));
    browser.storage.local
      .get(["locale", "theme", "themeMode"])
      .then((v) => {
        if (typeof v.locale === "string") setLocale(v.locale as Locale);
        // Older builds saved "light" automatically. Only an explicit new
        // preference, or the old non-default dark choice, is an override.
        if (v.themeMode === "manual" && (v.theme === "light" || v.theme === "dark"))
          setTheme(v.theme);
        else if (v.themeMode === undefined && v.theme === "dark")
          setTheme("dark");
      })
      .catch(() => {})
      .finally(() => setPrefsLoaded(true));
  }, []);
  useEffect(() => {
    if (!s?.ticket) return;
    const timer = window.setInterval(() => void load().catch(() => {}), 3000);
    return () => window.clearInterval(timer);
  }, [s?.ticket?.nonce, s?.paired]);
  useEffect(() => {
    document.documentElement.lang = locale;
    if (theme === "system") document.documentElement.removeAttribute("data-theme");
    else document.documentElement.dataset.theme = theme;
    if (prefsLoaded) void browser.storage.local.set({ locale });
  }, [locale, theme, prefsLoaded]);
  useEffect(() => {
    const syncSection = () => setSection(location.hash.slice(1) || "context");
    window.addEventListener("hashchange", syncSection);
    return () => window.removeEventListener("hashchange", syncSection);
  }, []);
  function go(v: string) {
    setSection(v);
    location.hash = v;
    setNotice("");
    setError("");
  }
  async function run(action: () => Promise<unknown>, success = "") {
    setBusy(true);
    setError("");
    try {
      await action();
      await load();
      if (success) setNotice(success);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  async function policy(patch: any) {
    await run(() => call({ type: "policy", patch }));
  }
  async function copySetup() {
    if (!s?.paired && (!consent || !disclosure)) return;
    if (!s?.paired)
      await call({
        type: "policy",
        patch: {
          consent: true,
          recall_enabled: true,
          selected_only: selectedOnly,
        },
      });
    const ticket = await call({
      type: "ticket",
      adapters: selected,
      install_skills: false,
      label:
        "Personal · " +
        (import.meta.env.BROWSER === "firefox" ? "Firefox" : "Chrome"),
    });
    const text = tr("setup.assistantInstructions", {
      ticket: JSON.stringify(ticket, null, 2),
    });
    setSetupText(text);
    await navigator.clipboard.writeText(text);
    setNotice(
      `${tr("setup.instructionsCopied")} ${tr("setup.instructionsPasteHint")}`,
    );
    await load();
  }
  async function copySkillInstallCommand() {
    if (!skillRepository) return;
    try {
      await navigator.clipboard.writeText(tr("connections.skillInstallPrompt", { repository: skillRepository }));
      setError("");
      setNotice(tr("connections.skillCommandCopied"));
    } catch (e) {
      setError((e as Error).message || tr("errors.generic"));
    }
  }
  const cards = (data.cards || []).filter(
    (c: any) =>
      (filter === "all" || c.state === filter) &&
      c.text.toLowerCase().includes(search.toLowerCase()),
  );
  return (
    <div class="shell">
      <aside class="sidebar">
        <div class="brand row">
          <Icon name="mark" size={32} />
          Serein
        </div>
        <nav class="nav" aria-label={tr("dashboard.navLabel")}>
          {nav.map(([key, icon]) => (
            <button
              class={section === key ? "active" : ""}
              aria-current={section === key ? "page" : undefined}
              onClick={() => go(key)}
            >
              <Icon name={icon} />
              {tr(("navigation." + key) as MessageKey)}
            </button>
          ))}
        </nav>
        <div class="sidebar-bottom">
          <p class="small">
            {tr("app.tagline").split(". ")[0]}.<br />
            {tr("app.tagline").split(". ")[1]}
          </p>
          <div class="row small" style={{ marginTop: 20, gap: 7 }}>
            <span class="dot" style={{ color: "var(--accent)" }} />
            {tr("dashboard.deviceLabel")}
          </div>
        </div>
      </aside>
      <main class="main">
        <div class="topbar">
          <span class="eyebrow">{tr("dashboard.eyebrow")}</span>
          <div class="row">
            <select
              aria-label={tr("dashboard.language")}
              disabled={!prefsLoaded}
              value={locale}
              onChange={(e) => setLocale(e.currentTarget.value as Locale)}
            >
              <option value="en">English</option>
              <option value="de">Deutsch</option>
              <option value="nl">Nederlands</option>
              <option value="zh-CN">简体中文</option>
              <option value="ja">日本語</option>
              <option value="es">Español</option>
            </select>
            <button
              class="ghost"
              aria-label={tr("dashboard.themeToggle")}
              disabled={!prefsLoaded}
              onClick={() => {
                const isDark = theme === "system"
                  ? window.matchMedia("(prefers-color-scheme: dark)").matches
                  : theme === "dark";
                const next = isDark ? "light" : "dark";
                setTheme(next);
                void browser.storage.local.set({ theme: next, themeMode: "manual" });
              }}
            >
              <Icon name="sun" size={18} />
            </button>
          </div>
        </div>
        {error && (
          <div role="alert" class="notice error" style={{ marginBottom: 20 }}>
            {error}
          </div>
        )}
        {notice && (
          <div role="status" class="notice" style={{ marginBottom: 20 }}>
            {notice}
          </div>
        )}
        {pending > 0 && (
          <div class="notice" style={{ marginBottom: 20 }}>
            {tr("dashboard.pendingPrivacy")}
          </div>
        )}
        {section === "context" && (
          <>
            <div class="row between page-heading">
              <div>
                <h1>{tr("context.heading")}</h1>
                <p>{tr("context.subtitle")}</p>
              </div>
              <span class="badge">
                <Icon name="device" size={14} />
                {tr("context.savedDevice")}
              </span>
            </div>
            <div class="toolbar">
              <div class="tabs" aria-label={tr("context.filterLabel")}>
                {["all", "observed", "confirmed"].map((f) => (
                  <button
                    aria-pressed={filter === f}
                    class={filter === f ? "active" : ""}
                    onClick={() => setFilter(f)}
                  >
                    {f === "all"
                      ? tr("context.filterAll")
                      : tr(("context." + f) as MessageKey)}
                  </button>
                ))}
              </div>
              <label class="search">
                <Icon name="search" size={16} />
                <input
                  aria-label={tr("context.searchLabel")}
                  placeholder={tr("context.searchPlaceholder")}
                  value={search}
                  onInput={(e) => setSearch(e.currentTarget.value)}
                />
              </label>
            </div>
            {cards.length ? (
              <div class="context-grid">
                {cards.map((card: any) => (
                  <article class="card context-card">
                    <div class="row between">
                      <span class="badge">
                        {card.state === "confirmed" ? (
                          <Icon name="check" size={13} />
                        ) : (
                          <span class="dot" />
                        )}
                        {tr(("context." + card.state) as MessageKey)}
                      </span>
                      <span class="small">
                        {new Date(card.last_seen).toLocaleDateString(locale, {
                          month: "short",
                          day: "numeric",
                        })}
                      </span>
                    </div>
                    <h3>{card.text}</h3>
                    <div class="row">
                      <span class="initial">
                        {card.site.slice(0, 2).toUpperCase()}
                      </span>
                      <div class="small">
                        {card.site}
                        <br />
                        {tr(
                          card.sessions === 1
                            ? "context.sessionSingular"
                            : "context.sessionsPlural",
                          { count: card.sessions },
                        )}{" "}
                        ·{" "}
                        {tr(
                          card.sites === 1
                            ? "context.siteSingular"
                            : "context.sitesPlural",
                          { count: card.sites },
                        )}
                      </div>
                    </div>
                    {card.corrections?.length > 0 && (
                      <p class="small">
                        {card.corrections
                          .map((x: any) =>
                            correctionLabels[x.action]
                              ? tr(correctionLabels[x.action])
                              : x.action.replaceAll("_", " "),
                          )
                          .join(" · ")}
                      </p>
                    )}
                    <footer>
                      <span class="small">{tr("context.sourceBacked")}</span>
                      <div class="row" style={{ gap: 2 }}>
                        <button onClick={() => setModal({ kind: "why", card })}>
                          {tr("context.why")}
                        </button>
                        <button
                          onClick={() => setModal({ kind: "correct", card })}
                        >
                          {tr("context.correct")}
                        </button>
                        <button
                          onClick={() => setModal({ kind: "forget", card })}
                        >
                          {tr("context.forget")}
                        </button>
                      </div>
                    </footer>
                  </article>
                ))}
              </div>
            ) : (
              <div class="card empty">
                <div class="symbol">
                  <Icon name="mark" size={42} />
                </div>
                <h2>
                  {search ? tr("context.noMatch") : tr("context.emptyTitle")}
                </h2>
                <p>
                  {s?.paired
                    ? tr("context.emptyConnected")
                    : tr("context.emptyDisconnected")}
                </p>
                <button
                  class="primary"
                  disabled={busy}
                  onClick={() => (s?.paired ? run(load) : go("connections"))}
                >
                  {s?.paired ? tr("context.refresh") : tr("context.connect")}
                  <Icon name="arrow" size={16} />
                </button>
              </div>
            )}
            <p class="section-note">
              <Icon name="why" size={16} />
              {tr("context.evidenceNote")}
            </p>
            {s?.paired && !data.model_available && (
              <div class="notice" style={{ marginTop: 20 }}>
                {tr("context.semanticUnavailable")}
              </div>
            )}
            {s?.paired && data.model_available && (
              <div class="notice" style={{ marginTop: 20 }}>
                {tr("context.semanticEnabled")}
              </div>
            )}
          </>
        )}
        {section === "connections" && (
          <div class="onboarding">
            <div class="page-heading">
              <h1>{tr("connections.heading")}</h1>
              <p>
                {s?.paired
                  ? tr("connections.readyDescription")
                  : tr("connections.getStartedDescription")}
              </p>
            </div>
            {!s?.paired && (
              <div class="card stack">
                <div class="row">
                  <Icon name="device" size={24} />
                  <h3>{tr("connections.localControlTitle")}</h3>
                </div>
                <p>{tr("connections.captureDetails")}</p>
                <label class="checklabel">
                  <input
                    type="checkbox"
                    checked={consent}
                    onChange={(e) => setConsent(e.currentTarget.checked)}
                  />
                  <span>{tr("connections.consentCapture")}</span>
                </label>
                <label class="checklabel">
                  <input
                    type="checkbox"
                    checked={disclosure}
                    onChange={(e) => setDisclosure(e.currentTarget.checked)}
                  />
                  <span>{tr("connections.consentRecallDetails")}</span>
                </label>
                <label>
                  {tr("connections.captureScope")}
                  <select
                    value={selectedOnly ? "selected" : "recommended"}
                    onChange={(e) =>
                      setSelectedOnly(e.currentTarget.value === "selected")
                    }
                  >
                    <option value="recommended">
                      {tr("connections.recommendedExclusions")}
                    </option>
                    <option value="selected">
                      {tr("privacy.selectedSitesOnly")}
                    </option>
                  </select>
                </label>
                <small>{tr("connections.filterLimitShort")}</small>
              </div>
            )}
            <details class="link-steps" open={!s?.paired}>
              <summary class="small">
                {s?.paired
                  ? tr("connections.linkAnother")
                  : tr("connections.setupSteps")}
              </summary>
            <div class="step">
              <span class="step-number">1</span>
              <h3>{tr("connections.chooseAssistants")}</h3>
            </div>
            <div class="connections">
              {assistants.map(([id, name, initial]) => (
                <button
                  class={
                    "assistant " + (selected.includes(id) ? "selected" : "")
                  }
                  aria-pressed={selected.includes(id)}
                  onClick={() => {
                    setSelected(
                      selected.includes(id)
                        ? selected.filter((x) => x !== id)
                        : [...selected, id],
                    );
                  }}
                >
                  <div class="row between" style={{ width: "100%" }}>
                    <span class="monogram">{initial}</span>
                    {selected.includes(id) && <Icon name="check" size={16} />}
                  </div>
                  <strong>{tr(name)}</strong>
                  <small>
                    {s?.lastStatus?.adapters?.some(
                      (adapter: any) =>
                        adapter?.id === id && adapter.installed === true,
                    )
                      ? tr("connections.executionNotTested")
                      : tr("connections.localExecutionRequired")}
                  </small>
                </button>
              ))}
            </div>
            <p class="small" style={{ marginTop: 12 }}>
              {tr("connections.remoteLimit")}
            </p>
            <div class="step">
              <span class="step-number">2</span>
              <h3>{tr("connections.installSkill")}</h3>
            </div>
            <div class="card">
              <p>{tr("connections.skillInstallDetails")}</p>
              {skillRepository ? (
                <div class="stack" style={{ gap: 12, marginTop: 16 }}>
                  <a
                    class="skill-source"
                    href={skillRepository}
                    target="_blank"
                    rel="noreferrer"
                  >
                    {skillRepository}
                  </a>
                  <small>{tr("connections.skillGlobalNote")}</small>
                  <div>
                    <button
                      class="primary"
                      onClick={() => void copySkillInstallCommand()}
                    >
                      <Icon name="copy" size={16} />
                      {tr("connections.copySkillCommand")}
                    </button>
                  </div>
                  {skillInstallCommand && (
                    <details>
                      <summary class="small">Skills CLI</summary>
                      <code class="path skill-command">{skillInstallCommand}</code>
                    </details>
                  )}
                </div>
              ) : (
                <p class="notice" style={{ marginTop: 16 }}>
                  {tr("connections.skillSourceUnconfigured")}
                </p>
              )}
            </div>
            <div class="step">
              <span class="step-number">3</span>
              <h3>{tr("connections.connectLocally")}</h3>
            </div>
            <div class="card">
              <h3>
                {s?.paired
                  ? tr("connections.connectedBrowser")
                  : tr("connections.helperTitle")}
              </h3>
              <p style={{ marginTop: 8 }}>{tr("connections.helperDetails")}</p>
              <div class="form-actions">
                <button
                  class="primary"
                  disabled={
                    busy ||
                    (!s?.paired && (!consent || !disclosure)) ||
                    !selected.length
                  }
                  onClick={() => run(copySetup)}
                >
                  <Icon name="copy" size={16} />
                  {tr("setup.copyInstructions")}
                </button>
              </div>
              {setupText && (
                <details style={{ marginTop: 16 }}>
                  <summary class="small">
                    {tr("connections.reviewInstructions")}
                  </summary>
                  <textarea rows={10} readOnly value={setupText} />
                </details>
              )}
              <small style={{ display: "block", marginTop: 16 }}>
                {tr("connections.helperSeparateFromSkill")}
              </small>
            </div>
            </details>
            {s?.paired && (
              <details class="card" style={{ marginTop: 20 }}>
                <summary>{tr("connections.advancedVault")}</summary>
                <p class="small" style={{ margin: "16px 0 8px" }}>
                  {tr("connections.actualDatabasePath")}
                </p>
                <div class="path">
                  {data.database_path || s.lastStatus?.database_path}
                </div>
                <button
                  style={{ marginTop: 12 }}
                  onClick={() =>
                    run(
                      async () =>
                        navigator.clipboard.writeText(
                          data.database_path || s.lastStatus?.database_path,
                        ),
                      tr("connections.databasePathCopied"),
                    )
                  }
                >
                  <Icon name="copy" size={15} />
                  {tr("connections.copyDatabasePath")}
                </button>
              </details>
            )}
          </div>
        )}
        {section === "privacy" && (
          <>
            <div class="page-heading">
              <h1>{tr("navigation.privacy")}</h1>
              <p>{tr("privacy.pageDescription")}</p>
            </div>
            <div class="card">
              <Setting
                title={tr("privacy.capture")}
                description={tr("privacy.captureDescription")}
              >
                <Toggle
                  label={tr("privacy.capture")}
                  checked={!!s?.policy.consent && !s?.policy.paused}
                  onChange={() =>
                    run(() =>
                      call({ type: "pause", paused: !s?.policy.paused }),
                    )
                  }
                />
              </Setting>
              <Setting
                title={tr("privacy.assistantRecall")}
                description={tr("privacy.recallDescription")}
              >
                <Toggle
                  label={tr("privacy.assistantRecall")}
                  checked={!!s?.policy.recall_enabled}
                  onChange={() =>
                    policy({ recall_enabled: !s?.policy.recall_enabled })
                  }
                />
              </Setting>
              <Setting
                title={tr("privacy.selectedSitesOnly")}
                description={tr("privacy.onlySelectedDescription")}
              >
                <Toggle
                  label={tr("privacy.selectedSitesOnly")}
                  checked={!!s?.policy.selected_only}
                  onChange={() =>
                    policy({ selected_only: !s?.policy.selected_only })
                  }
                />
              </Setting>
            </div>
            <div class="card" style={{ marginTop: 20 }}>
              <h3>
                {s?.policy.selected_only
                  ? tr("privacy.selectedSiteMode")
                  : tr("privacy.excludedSiteMode")}
              </h3>
              <p class="small" style={{ marginTop: 8 }}>
                {tr("privacy.ruleScope")}
              </p>
              <div class="form-actions">
                <input
                  style={{ flex: 1, minWidth: 180 }}
                  aria-label={tr("privacy.hostnameLabel")}
                  placeholder={tr("privacy.hostnamePlaceholder")}
                  value={site}
                  onInput={(e) =>
                    setSite(e.currentTarget.value.toLowerCase().trim())
                  }
                />
                <button
                  disabled={!validSite(site) || busy}
                  onClick={() =>
                    run(async () => {
                      await call({
                        type: s?.policy.selected_only ? "include" : "exclude",
                        site,
                        forget: false,
                      });
                      setSite("");
                    })
                  }
                >
                  {s?.policy.selected_only
                    ? tr("privacy.includeSite")
                    : tr("privacy.excludeSite")}
                </button>
              </div>
              {(s?.policy.selected_only
                ? s.policy.selected_sites
                : s?.policy.excluded_sites || []
              ).map((hostname) => (
                <div class="setting">
                  <span>{hostname}</span>
                  <button
                    onClick={() =>
                      policy(
                        s?.policy.selected_only
                          ? {
                              selected_sites: s.policy.selected_sites.filter(
                                (x) => x !== hostname,
                              ),
                            }
                          : {
                              excluded_sites: s?.policy.excluded_sites.filter(
                                (x) => x !== hostname,
                              ),
                            },
                      )
                    }
                  >
                    {tr("privacy.removeRule")}
                  </button>
                </div>
              ))}
            </div>
            <div class="notice" style={{ marginTop: 20 }}>
              {tr("privacy.sensitiveNotice")}
            </div>
            <div class="card" style={{ marginTop: 20 }}>
              <div class="row between">
                <div>
                  <h3>{tr("privacy.receipts")}</h3>
                  <p class="small">{tr("privacy.receiptsDescription")}</p>
                </div>
                <button
                  disabled={!s?.paired}
                  onClick={() =>
                    run(async () =>
                      setReceipt((await host("receipts")).receipts),
                    )
                  }
                >
                  {tr("privacy.viewReceipts")}
                </button>
              </div>
              {receipt.map((r) => (
                <div class="setting small">
                  <span>
                    {new Date(r.time).toLocaleString()} · {r.status}
                  </span>
                  <span>
                    {tr("privacy.receiptDetails", {
                      records: r.ids.length,
                      bytes: r.bytes,
                    })}
                  </span>
                </div>
              ))}
            </div>
          </>
        )}
        {section === "storage" && (
          <>
            <div class="page-heading">
              <h1>{tr("navigation.storage")}</h1>
              <p>{tr("storage.pageDescription")}</p>
            </div>
            <div class="stats">
              {[
                [
                  "storage.vaultStat",
                  data.vault_bytes
                    ? `${(data.vault_bytes / 1048576).toFixed(1)} MB`
                    : tr("storage.notConnected"),
                ],
                [
                  "storage.queueStat",
                  tr("storage.queuedEvents", { count: queue }),
                ],
                [
                  "storage.modelStat",
                  data.model_available
                    ? tr("storage.modelInstalled")
                    : tr("storage.notInstalled"),
                ],
                ["storage.diagnosticsStat", tr("storage.metadataOnly")],
              ].map(([key, value]) => (
                <div class="card">
                  <small>{tr(key as MessageKey)}</small>
                  <strong
                    style={{
                      fontSize:
                        key === "storage.diagnosticsStat" ||
                        key === "storage.modelStat"
                          ? 16
                          : 24,
                    }}
                  >
                    {value}
                  </strong>
                </div>
              ))}
            </div>
            <div class="card" style={{ marginTop: 20 }}>
              <Setting
                title={tr("storage.browsingObservations")}
                description={tr("storage.retentionNinetyDays")}
              >
                <span class="badge">{tr("storage.retentionNinetyDays")}</span>
              </Setting>
              <Setting
                title={tr("storage.eventRetentionTitle")}
                description={tr("storage.eventRetentionDescription")}
              >
                <span class="badge">{tr("storage.retentionSevenDays")}</span>
              </Setting>
              <Setting
                title={tr("storage.constraintsTitle")}
                description={tr("storage.constraintsDescription")}
              >
                <span class="badge">{tr("storage.retentionUntilRemoved")}</span>
              </Setting>
            </div>
            <div class="card" style={{ marginTop: 20 }}>
              <h3>{tr("storage.dataOnYourTerms")}</h3>
              <p class="small" style={{ marginTop: 8 }}>
                {tr("storage.exportDescription")}
              </p>
              <div class="form-actions">
                <button
                  disabled={!s?.paired}
                  onClick={() => setModal({ kind: "export" })}
                >
                  {tr("storage.exportReviewedContext")}
                </button>
                <button
                  class="danger"
                  disabled={!s?.paired}
                  onClick={() => setModal({ kind: "erase" })}
                >
                  <Icon name="forget" size={16} />
                  {tr("storage.eraseSavedContext")}
                </button>
              </div>
            </div>
            <p class="section-note">
              {tr("storage.deletionLimit", { count: s?.dropped || 0 })}
            </p>
          </>
        )}
        {section === "about" && (
          <>
            <div class="page-heading">
              <h1>{tr("about.quieterContext")}</h1>
              <p>{tr("about.description")}</p>
            </div>
            <div class="card stack">
              <div class="brand row">
                <Icon name="mark" size={40} />
                Serein{" "}
                <span class="badge">{tr("about.developmentVersion")}</span>
              </div>
              <p>{tr("app.tagline")}</p>
              <hr />
              <h3>{tr("about.filesAtRest")}</h3>
              <p>{tr("about.filesDescription")}</p>
              <h3>{tr("about.evidenceTitle")}</h3>
              <p>{tr("about.evidenceDescription")}</p>
              <h3>{tr("about.buildStatus")}</h3>
              <p>{tr("about.buildDescription")}</p>
              <small>{tr("about.licenseNote")}</small>
            </div>
          </>
        )}
        <footer class="footer-note row between">
          <span>{tr("dashboard.footer")}</span>
          <span>
            {s?.paired
              ? tr("dashboard.localHelperConnected")
              : tr("dashboard.finishSetup")}
          </span>
        </footer>
      </main>
      {modal && (
        <Modal
          title={
            modal.kind === "why"
              ? tr("modal.whyTitle")
              : modal.kind === "correct"
                ? tr("modal.correctTitle")
                : modal.kind === "export"
                  ? tr("modal.exportTitle")
                  : modal.kind === "erase"
                    ? tr("modal.eraseTitle")
                    : tr("modal.forgetTitle")
          }
          onClose={() => setModal(null)}
        >
          {modal.kind === "why" ? (
            <div class="stack">
              <p>{modal.card.text}</p>
              <div class="notice">
                {tr("modal.observedOn", { site: modal.card.site })}
                <br />
                {tr("modal.sessionSummary", {
                  sessions: modal.card.sessions,
                  time: new Date(modal.card.last_seen).toLocaleString(locale),
                })}
              </div>
              <p>{tr("modal.attentionExplanation")}</p>
            </div>
          ) : modal.kind === "correct" ? (
            <div class="stack">
              {[
                ["not_about_me", "correction.notAboutMe"],
                ["wrong_topic", "correction.wrongTopic"],
                ["temporary_research", "correction.temporaryResearch"],
                ["do_not_use", "correction.doNotUse"],
              ].map(([action, key]) => (
                <button
                  disabled={busy}
                  style={{ justifyContent: "flex-start" }}
                  onClick={() =>
                    run(async () => {
                      await host("feedback", {
                        atom_id: modal.card.id,
                        action,
                        text: null,
                      });
                      setModal(null);
                    }, tr("correction.saved"))
                  }
                >
                  {tr(key as MessageKey)}
                </button>
              ))}
              <label class="small">
                {tr("modal.constraintLabel")}
                <textarea
                  placeholder={tr("modal.constraintExample")}
                  value={constraint}
                  onInput={(e) => setConstraint(e.currentTarget.value)}
                  maxLength={512}
                />
              </label>
              <button
                class="primary"
                disabled={busy || !constraint.trim()}
                onClick={() =>
                  run(async () => {
                    await host("feedback", {
                      atom_id: modal.card.id,
                      action: "confirm_constraint",
                      text: constraint,
                    });
                    setModal(null);
                    setConstraint("");
                  }, tr("modal.constraintConfirmed"))
                }
              >
                {tr("correction.confirmConstraint")}
              </button>
            </div>
          ) : modal.kind === "export" ? (
            <>
              <p>
                {tr("modal.exportDescription", {
                  count: data.cards?.length || 0,
                })}
              </p>
              <button
                class="primary"
                style={{ marginTop: 20 }}
                onClick={() => {
                  const a = document.createElement("a");
                  a.href = URL.createObjectURL(
                    new Blob(
                      [
                        JSON.stringify(
                          {
                            exported_at: new Date().toISOString(),
                            observations: data.cards,
                          },
                          null,
                          2,
                        ),
                      ],
                      { type: "application/json" },
                    ),
                  );
                  a.download = "serein-reviewed-context.json";
                  a.click();
                  URL.revokeObjectURL(a.href);
                  setModal(null);
                }}
              >
                {tr("modal.exportJson")}
              </button>
            </>
          ) : (
            <>
              <p>
                {modal.kind === "erase"
                  ? tr("modal.eraseDescription")
                  : tr("modal.forgetDescription")}
              </p>
              <div class="form-actions">
                <button onClick={() => setModal(null)}>
                  {tr("modal.cancel")}
                </button>
                <button
                  class="danger"
                  disabled={busy}
                  onClick={() =>
                    run(async () => {
                      if (modal.kind === "erase")
                        await call({ type: "pause", paused: true });
                      await host("forget", {
                        site: null,
                        atom_id: modal.card?.id || null,
                        site_epoch: (s?.policy.capture_epoch || 0) + 1,
                      });
                      setModal(null);
                    }, tr("modal.savedEvidenceRemoved"))
                  }
                >
                  {modal.kind === "erase"
                    ? tr("modal.eraseAndPause")
                    : tr("modal.forgetObservation")}
                </button>
              </div>
            </>
          )}
        </Modal>
      )}
    </div>
  );
}
render(<App />, document.getElementById("app")!);

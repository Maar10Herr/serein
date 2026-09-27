import type { Policy, Observation } from "./types";
export const matches = (site: string, rule: string) =>
  site === rule || site.endsWith("." + rule);
export function validSite(site: string) {
  return (
    site.length <= 253 &&
    site.includes(".") &&
    /^[a-z0-9.-]+$/.test(site) &&
    !/^\d+(\.\d+){3}$/.test(site) &&
    !site.split(".").some((x) => !x || x.startsWith("-") || x.endsWith("-")) &&
    !["localhost", "local", "internal", "test", "invalid", "onion"].some((x) =>
      matches(site, x),
    )
  );
}
const blocked = [
  "mail.google.com",
  "outlook.live.com",
  "outlook.office.com",
  "proton.me",
  "mail.yahoo.com",
  "web.whatsapp.com",
  "messenger.com",
  "web.telegram.org",
  "pornhub.com",
  "xvideos.com",
  "paypal.com",
  "chase.com",
  "mychart.com",
  "accounts.google.com",
  "login.microsoftonline.com",
];
export function sensitive(s: string) {
  return [
    "porn",
    "sex",
    "suicide",
    "diagnos",
    "cancer",
    "oncolog",
    "patient",
    "medical record",
    "bank account",
    "inbox",
    "password",
    "passwort",
    "wachtwoord",
    "mot de passe",
    "religio",
    "politic",
    "politiek",
    "politisch",
    "election",
    "verkiezing",
    "wahlkampf",
    "depress",
    "hiv",
    "pregnan",
    "schwanger",
    "zwanger",
    "geestelijke",
    "santé",
    "病",
    "政治",
    "宗教",
    "健康",
    "診断",
  ].some((x) => s.toLowerCase().includes(x));
}
export function clean(s: string, max: number) {
  return Array.from(
    s
      .normalize("NFKC")
      .replace(/[\u0000-\u001f\u007f]/g, "")
      .replace(/[\w.+-]+@[\w.-]+\.[a-z]{2,}/gi, "[redacted email]")
      .replace(
        /(bearer\s+\S+|(?:token|password|session|secret|api[_ -]?key)\s*[:=]\s*\S+|\b[a-z0-9_-]{40,}\b)/gi,
        "[redacted]",
      )
      .replace(/\+?\d[\d ()-]{8,}\d/g, "[redacted number]"),
  )
    .slice(0, max)
    .join("")
    .trim();
}
const google = [
  "google.com",
  "google.co.uk",
  "google.de",
  "google.nl",
  "google.fr",
  "google.co.jp",
  "google.ca",
  "google.com.au",
];
const amazon = [
  "amazon.com",
  "amazon.co.uk",
  "amazon.de",
  "amazon.nl",
  "amazon.fr",
  "amazon.co.jp",
  "amazon.ca",
  "amazon.com.au",
  "amazon.it",
  "amazon.es",
];
export function searchTerm(url: URL) {
  const host = url.hostname.replace(/^www\./, "");
  let param: string | undefined;
  if (
    (google.includes(host) && url.pathname === "/search") ||
    (host === "bing.com" && url.pathname === "/search") ||
    (["duckduckgo.com", "search.brave.com", "kagi.com"].includes(host) &&
      ["/", "/search"].includes(url.pathname))
  )
    param = "q";
  if (amazon.includes(host) && url.pathname === "/s") param = "k";
  if (host === "youtube.com" && url.pathname === "/results")
    param = "search_query";
  if (!param) return null;
  const values = url.searchParams.getAll(param);
  return values.length === 1 ? clean(values[0], 512) || null : null;
}
export function sanitize(
  tab: { url?: string; title?: string; incognito?: boolean },
  policy: Policy,
): Omit<
  Observation,
  "event_id" | "visit_id" | "site_epoch" | "observed_at" | "foreground_seconds"
> | null {
  if (!policy.consent || policy.paused || tab.incognito || !tab.url)
    return null;
  let u: URL;
  try {
    u = new URL(tab.url);
  } catch {
    return null;
  }
  const site = u.hostname.toLowerCase();
  if (
    !["https:", "http:"].includes(u.protocol) ||
    u.username ||
    u.password ||
    !validSite(site) ||
    blocked.some((x) => matches(site, x)) ||
    site
      .split(".")
      .some((x) =>
        ["banking", "webmail", "login", "accounts", "patient", "auth"].includes(
          x,
        ),
      ) ||
    /(^|\/)(login|signin|sign-in|auth|account|checkout|patient|inbox|messages)(\/|$)/i.test(
      u.pathname,
    ) ||
    policy.excluded_sites.some((x) => matches(site, x)) ||
    (policy.selected_only &&
      !policy.selected_sites.some((x) => matches(site, x)))
  )
    return null;
  const title = clean(tab.title || "", 256);
  const query = searchTerm(u);
  if (
    sensitive(title) ||
    (query && sensitive(query)) ||
    new TextEncoder().encode(title + (query || "")).length > 2048
  )
    return null;
  return {
    site_key: site,
    title,
    search_query: query,
    kind: query ? "search" : "visit",
  };
}

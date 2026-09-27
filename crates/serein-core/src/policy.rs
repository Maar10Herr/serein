use crate::*;
use regex::Regex;
use unicode_normalization::UnicodeNormalization;
pub fn matches(site: &str, rule: &str) -> bool {
    site == rule || site.strip_suffix(rule).is_some_and(|x| x.ends_with('.'))
}
pub fn valid_site(s: &str) -> bool {
    s.len() <= 253
        && s.contains('.')
        && s == s.to_lowercase()
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-')
        && !s
            .split('.')
            .any(|x| x.is_empty() || x.starts_with('-') || x.ends_with('-'))
        && !s.parse::<std::net::IpAddr>().is_ok()
        && !["localhost", "local", "internal", "test", "invalid", "onion"]
            .iter()
            .any(|x| matches(s, x))
}
pub fn sensitive(s: &str) -> bool {
    let s = s.to_lowercase();
    [
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
    ]
    .iter()
    .any(|w| s.contains(w))
}
pub fn blocked_site(s: &str) -> bool {
    [
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
    ]
    .iter()
    .any(|r| matches(s, r))
        || s.split('.')
            .any(|x| ["banking", "webmail", "login", "accounts", "patient", "auth"].contains(&x))
}
pub fn clean(s: &str, max: usize) -> String {
    let s: String = s.nfkc().filter(|c| !c.is_control()).collect();
    let email = Regex::new(r"(?i)[\w.+-]+@[\w.-]+\.[a-z]{2,}").unwrap();
    let s = email.replace_all(&s, "[redacted email]");
    let token=Regex::new(r"(?i)(bearer\s+\S+|(?:token|password|session|secret|api[_ -]?key)\s*[:=]\s*\S+|\b[a-z0-9_-]{40,}\b)").unwrap();
    let s = token.replace_all(&s, "[redacted]");
    let phone = Regex::new(r"\+?\d[\d ()-]{8,}\d").unwrap();
    phone
        .replace_all(&s, "[redacted number]")
        .chars()
        .take(max)
        .collect::<String>()
        .trim()
        .to_string()
}
pub fn allowed(e: &Event, p: &Policy) -> std::result::Result<Event, &'static str> {
    if !p.consent || p.paused {
        return Err("CAPTURE_DISABLED");
    }
    if !valid_site(&e.site_key) || blocked_site(&e.site_key) {
        return Err("EXCLUDED_SITE");
    }
    if p.excluded_sites.iter().any(|r| matches(&e.site_key, r))
        || p.selected_only && !p.selected_sites.iter().any(|r| matches(&e.site_key, r))
    {
        return Err("EXCLUDED_SITE");
    }
    if sensitive(&e.title) || e.search_query.as_ref().is_some_and(|x| sensitive(x)) {
        return Err("SENSITIVE_METADATA");
    }
    if e.kind == "visit" && e.foreground_seconds < 5 {
        return Err("SHORT_VISIT");
    }
    let mut e = e.clone();
    e.title = clean(&e.title, 256);
    e.search_query = e.search_query.map(|x| clean(&x, 512));
    Ok(e)
}

//! Conservative presentation rules for captured browser metadata.
//! This is a display classification, not a claim about a user's interests.

fn normalized_title(title: &str) -> String {
    title
        .to_lowercase()
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn generic_title(title: &str) -> bool {
    matches!(
        normalized_title(title).as_str(),
        "home"
            | "home x"
            | "x home"
            | "x"
            | "x twitter"
            | "twitter x"
            | "twitter"
            | "twitter home"
            | "home twitter"
            | "explore x"
            | "x explore"
            | "for you x"
            | "x for you"
            | "feed x"
            | "x feed"
            | "explore twitter"
            | "twitter explore"
            | "for you twitter"
            | "twitter for you"
            | "feed twitter"
            | "twitter feed"
            | "youtube"
            | "youtube com"
            | "youtube home"
            | "home youtube"
            | "google"
            | "google search"
            | "new tab"
            | "new tab page"
            | "start page"
            | "reddit"
            | "reddit home"
            | "home reddit"
            | "feed reddit"
            | "reddit feed"
            | "instagram"
            | "instagram home"
            | "facebook"
            | "facebook home"
            | "tiktok"
            | "tiktok home"
            | "linkedin"
            | "linkedin home"
            | "home linkedin"
            | "linkedin feed"
            | "feed linkedin"
            | "explore"
            | "for you"
            | "feed"
            | "weather"
            | "calendar"
            | "blank"
    )
}

pub fn informative_title(title: &str) -> bool {
    if generic_title(title) || normalized_title(title).starts_with("inbox ") {
        return false;
    }
    let alphanumeric = title.chars().filter(|ch| ch.is_alphanumeric()).count();
    alphanumeric >= 7
}

fn meaningful_search(query: &str) -> bool {
    let parts: Vec<_> = query
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .collect();
    parts.len() >= 2
        || (!query.is_ascii() && query.chars().filter(|ch| ch.is_alphanumeric()).count() >= 5)
}

pub fn prominent(
    title: &str,
    search_query: Option<&str>,
    foreground_seconds: i64,
    sessions: i64,
    user_confirmed: bool,
) -> bool {
    if user_confirmed {
        return true;
    }
    if let Some(query) = search_query.filter(|query| !query.trim().is_empty()) {
        return meaningful_search(query) && (foreground_seconds >= 10 || sessions >= 2);
    }
    informative_title(title) && sessions >= 2
}

pub fn groupable(title: &str, search_query: Option<&str>) -> bool {
    search_query.is_some_and(meaningful_search) || informative_title(title)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_feed_does_not_become_a_research_memory_by_repetition() {
        for title in [
            "Home / X",
            "Explore | X",
            "For You | X",
            "Twitter",
            "LinkedIn",
            "LinkedIn Feed",
            "YouTube.com",
            "X (Twitter)",
        ] {
            assert!(generic_title(title), "{title}");
            assert!(!prominent(title, None, 3600, 6, false), "{title}");
            assert!(!groupable(title, None), "{title}");
        }
        assert!(prominent("Home / X", None, 5, 1, true));
    }

    #[test]
    fn descriptive_pages_need_independent_sessions_for_standalone_prominence() {
        let title = "Steelcase Leap seat depth and dimensions";
        assert!(!prominent(title, None, 15, 1, false));
        assert!(!prominent(title, None, 300, 1, false));
        assert!(prominent(title, None, 15, 2, false));
        assert!(groupable("Aeron Chair", None));
        assert!(groupable("Alhambra", None));
        assert!(!groupable("Home / X", None));
    }

    #[test]
    fn search_and_non_latin_titles_can_be_useful() {
        assert!(prominent(
            "Google",
            Some("ergonomic office chair"),
            12,
            1,
            false
        ));
        assert!(!prominent(
            "ベランダで育てるミニトマトの詳しい方法",
            None,
            60,
            1,
            false
        ));
        assert!(groupable("ベランダで育てるミニトマトの詳しい方法", None));
        assert!(!prominent("Google", Some("office"), 4, 1, false));
    }
}

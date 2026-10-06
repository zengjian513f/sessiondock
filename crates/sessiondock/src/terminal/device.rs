//! Coarse device label from a browser `User-Agent` for ownership prompts.
//! A browser exposes neither host name nor login, so "iPhone · Safari" is the
//! most a takeover notice can say about who took the terminal. Display only.

/// `"<platform> · <browser>"` with unknown parts dropped; empty for a
/// non-browser client. Order matters: Chrome carries `Safari/`, Edge and
/// Opera carry `Chrome/`, so the more specific brand is checked first.
pub fn device_label(user_agent: &str) -> String {
    let platform = [
        ("iPhone", "iPhone"),
        ("iPad", "iPad"),
        ("Android", "Android"),
        ("Windows", "Windows"),
        ("CrOS", "ChromeOS"),
        ("Macintosh", "macOS"),
        ("Mac OS X", "macOS"),
        ("Linux", "Linux"),
    ]
    .iter()
    .find(|(needle, _)| user_agent.contains(needle))
    .map(|(_, label)| *label);
    let browser = [
        ("MicroMessenger/", "微信"),
        ("EdgiOS/", "Edge"),
        ("EdgA/", "Edge"),
        ("Edg/", "Edge"),
        ("OPR/", "Opera"),
        ("Opera", "Opera"),
        ("SamsungBrowser/", "Samsung Internet"),
        ("FxiOS/", "Firefox"),
        ("Firefox/", "Firefox"),
        ("CriOS/", "Chrome"),
        ("Chrome/", "Chrome"),
        ("Safari/", "Safari"),
    ]
    .iter()
    .find(|(needle, _)| user_agent.contains(needle))
    .map(|(_, label)| *label);
    [platform, browser]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
}

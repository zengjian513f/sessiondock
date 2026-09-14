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

#[cfg(test)]
mod tests {
    use super::device_label;

    #[test]
    fn labels_common_browsers_by_platform_and_brand() {
        let cases = [
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36",
                "Windows · Chrome",
            ),
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36 Edg/140.0.0.0",
                "Windows · Edge",
            ),
            (
                "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1",
                "iPhone · Safari",
            ),
            (
                "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/140.0.0.0 Mobile/15E148 Safari/604.1",
                "iPhone · Chrome",
            ),
            (
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15",
                "macOS · Safari",
            ),
            (
                "Mozilla/5.0 (X11; Linux x86_64; rv:130.0) Gecko/20100101 Firefox/130.0",
                "Linux · Firefox",
            ),
            (
                "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Mobile Safari/537.36",
                "Android · Chrome",
            ),
            (
                "Mozilla/5.0 (Linux; Android 14; 2312DRA50C Build/UKQ1.231003.002; wv) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/122.0.0.0 Mobile Safari/537.36 XWEB/1220 MMWEBSDK/20240301 MMWEBID/1234 MicroMessenger/8.0.48.2580(0x28003036) WeChat/arm64",
                "Android · 微信",
            ),
        ];
        for (agent, expected) in cases {
            assert_eq!(device_label(agent), expected, "{agent}");
        }
    }

    #[test]
    fn unknown_or_absent_agents_yield_an_empty_label() {
        assert_eq!(device_label(""), "");
        assert_eq!(device_label("curl/8.5.0"), "");
        assert_eq!(device_label("python-requests/2.32"), "");
        assert_eq!(
            device_label("Mozilla/5.0 (Windows NT 10.0; Win64; x64)"),
            "Windows"
        );
    }
}

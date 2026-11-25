use regex::Regex;
use std::sync::{LazyLock, PoisonError, RwLock};

static COOKIE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)_(?:\||%7c)WARNING(?::|%3a)[^\s;"',]+"#).unwrap());
static HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)(\.roblosecurity|\b(?:cookie|set-cookie|x-csrf-token|authorization))(["']?\s*[:=]\s*)("(?:[^"\\]|\\.)*"|'[^']*'|[^\r\n]+)"#,
    )
    .unwrap()
});
static SECRETS: RwLock<Vec<String>> = RwLock::new(Vec::new());
/// The home folder holds the Windows user name; logs may be attached to bug reports.
// ponytail: the plain and `{:?}`-escaped forms only (any case on Windows).
static HOME: LazyLock<Option<Regex>> = LazyLock::new(|| {
    let h = dirs::home_dir()?.display().to_string();
    (h.len() > 3).then(|| home_re(&h))
});

/// The home folder, only where its name ends: `/home/al` must not eat `/home/alex`. A
/// non-word character after it (a separator, a quote, a space) or the end of the text.
fn home_re(h: &str) -> Regex {
    let ci = if cfg!(windows) { "(?i)" } else { "" };
    let (esc, plain) = (regex::escape(&h.replace('\\', "\\\\")), regex::escape(h));
    // `C:/Users/x` (URLs, webview errors) too.
    let fwd = regex::escape(&h.replace('\\', "/"));
    Regex::new(&format!(r"{ci}(?:{esc}|{plain}|{fwd})(\W|$)")).unwrap()
}

const MASK: &str = "[REDACTED]";

/// Add a runtime secret (e.g. the local API token) that must never reach a log line.
pub fn register_secret(s: &str) {
    if !s.is_empty() {
        SECRETS
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .push(s.to_owned());
    }
}

pub fn redact(input: &str) -> String {
    let mut out = HEADER
        .replace_all(input, format!("$1$2{MASK}"))
        .into_owned();
    out = COOKIE.replace_all(&out, MASK).into_owned();
    // Runs in the panic hook: a poisoned lock must not abort before crash.log is written.
    for s in SECRETS
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
    {
        out = out.replace(s.as_str(), MASK);
    }
    if let Some(h) = HOME.as_ref() {
        out = h.replace_all(&out, "~$1").into_owned();
    }
    out
}

pub fn plugin<R: tauri::Runtime>(logs: std::path::PathBuf) -> tauri::plugin::TauriPlugin<R> {
    use tauri_plugin_log::{RotationStrategy, Target, TargetKind};
    tauri_plugin_log::Builder::new()
        .clear_targets()
        .target(Target::new(TargetKind::Folder {
            path: logs,
            file_name: Some("rmi".into()),
        }))
        .target(Target::new(TargetKind::Stdout))
        .level(log::LevelFilter::Info)
        // It logs ERROR on every failed check (no release feed yet); shell.rs logs checks at
        // info and install failures at warn itself.
        .level_for("tauri_plugin_updater", log::LevelFilter::Off)
        .max_file_size(5_000_000)
        .rotation_strategy(RotationStrategy::KeepSome(3))
        .format(|out, msg, rec| {
            out.finish(format_args!(
                "{} [{}] {}: {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
                rec.level(),
                rec.target(),
                redact(&msg.to_string())
            ))
        })
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_cookie_value() {
        let r = redact("got _|WARNING:-DO-NOT-SHARE-THIS.--abc123XYZ end");
        assert!(!r.contains("abc123"), "{r}");
        assert!(!r.contains("DO-NOT-SHARE"), "{r}");
        assert!(r.contains(MASK) && r.ends_with(" end"));
    }

    #[test]
    fn masks_headers() {
        assert_eq!(
            redact("Cookie: .ROBLOSECURITY=foo; a=b"),
            format!("Cookie: {MASK}")
        );
        assert_eq!(
            redact("x-csrf-token: 9f8e7d"),
            format!("x-csrf-token: {MASK}")
        );
        assert_eq!(
            redact("Authorization: Bearer abc"),
            format!("Authorization: {MASK}")
        );
    }

    #[test]
    fn masks_quoted_and_json_forms() {
        for (input, keep) in [
            (r#"{"x-csrf-token": "abc123", "n": 1}"#, r#""n": 1"#),
            (r#"{"cookie": "sekret.value"}"#, "}"),
            (r#"{'Cookie': 'sekret.value'}"#, "}"),
            (r#"headers { Cookie: "sekret.value" } ok"#, "} ok"),
        ] {
            let r = redact(input);
            assert!(!r.contains("abc123") && !r.contains("sekret"), "{r}");
            assert!(r.contains(MASK) && r.contains(keep), "{r}");
        }
    }

    #[test]
    fn masks_url_encoded_cookie() {
        for c in ["_%7CWARNING%3Afoo%2Dbar", "_%7cwarning%3afoo%2Dbar"] {
            let r = redact(&format!("url?c={c}&x=1"));
            assert!(!r.contains("foo"), "{r}");
            assert!(r.contains(MASK), "{r}");
        }
    }

    #[test]
    fn short_secret_masked() {
        register_secret("zq9");
        assert_eq!(redact("a zq9 b"), format!("a {MASK} b"));
    }

    #[test]
    fn masks_registered_secret() {
        register_secret("tok_0123456789abcdef");
        assert_eq!(
            redact("token=tok_0123456789abcdef!"),
            format!("token={MASK}!")
        );
    }

    #[test]
    fn masks_home_folder() {
        let home = dirs::home_dir().unwrap();
        let user = home.file_name().unwrap().to_string_lossy().into_owned();
        let p = home.join("AppData").join("rmi.log");
        for line in [format!("kept as {}", p.display()), format!("kept as {p:?}")] {
            let r = redact(&line);
            assert!(r.starts_with("kept as ") && r.contains('~'), "{r}");
            assert!(!r.contains(&user), "{r}");
        }
        // Only where the name ends: a longer folder name is someone else's.
        let re = home_re("/home/al");
        let m = |s: &str| re.replace_all(s, "~$1").into_owned();
        assert_eq!(m("/home/alex/x"), "/home/alex/x");
        assert_eq!(m("in /home/al/x and /home/al"), "in ~/x and ~");
        assert_eq!(m("\"/home/al\" ok"), "\"~\" ok");
        if cfg!(windows) {
            let re = home_re(r"C:\Users\Al");
            let m = |s: &str| re.replace_all(s, "~$1").into_owned();
            assert_eq!(m(r"c:\users\al\x"), r"~\x");
            assert_eq!(m(r#""C:\\USERS\\AL\\x""#), r#""~\\x""#);
            assert_eq!(m(r"C:\Users\Alex\x"), r"C:\Users\Alex\x");
        }
    }

    #[test]
    fn poisoned_secrets_still_redact() {
        let _ = std::thread::spawn(|| {
            let _g = SECRETS.write().unwrap_or_else(PoisonError::into_inner);
            panic!("poison");
        })
        .join();
        register_secret("pz7k");
        assert_eq!(redact("a pz7k b"), format!("a {MASK} b"));
    }

    #[test]
    fn masks_bare_roblosecurity_and_forward_slash_home() {
        assert_eq!(
            redact("bad .ROBLOSECURITY=abc"),
            format!("bad .ROBLOSECURITY={MASK}")
        );
        if cfg!(windows) {
            let re = home_re(r"C:\Users\Al");
            assert_eq!(
                re.replace_all("file:///C:/Users/Al/x", "~$1"),
                "file:///~/x"
            );
        }
    }

    #[test]
    fn leaves_plain_text() {
        assert_eq!(
            redact("launched mainAlt into Doors"),
            "launched mainAlt into Doors"
        );
    }
}

use regex::Regex;
use std::sync::{LazyLock, RwLock};

static COOKIE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)_(?:\||%7c)WARNING(?::|%3a)[^\s;"',]+"#).unwrap());
static HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)\b(cookie|set-cookie|x-csrf-token|authorization)(["']?\s*[:=]\s*)("(?:[^"\\]|\\.)*"|'[^']*'|[^\r\n]+)"#,
    )
    .unwrap()
});
static SECRETS: RwLock<Vec<String>> = RwLock::new(Vec::new());

const MASK: &str = "[REDACTED]";

/// Add a runtime secret (e.g. the local API token) that must never reach a log line.
pub fn register_secret(s: &str) {
    if !s.is_empty() {
        SECRETS.write().unwrap().push(s.to_owned());
    }
}

pub fn redact(input: &str) -> String {
    let mut out = HEADER
        .replace_all(input, format!("$1$2{MASK}"))
        .into_owned();
    out = COOKIE.replace_all(&out, MASK).into_owned();
    for s in SECRETS.read().unwrap().iter() {
        out = out.replace(s.as_str(), MASK);
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
    fn leaves_plain_text() {
        assert_eq!(
            redact("launched mainAlt into Doors"),
            "launched mainAlt into Doors"
        );
    }
}

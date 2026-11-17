//! Typed, versioned `settings.json` (no secrets). Stored as JSON merged over defaults so
//! unknown keys survive upgrades; reads go through JSON pointers (`/launch/delayMs`).

use serde_json::{json, Map, Value};
use std::path::Path;

pub const VERSION: u64 = MIGRATIONS.len() as u64;

pub fn defaults() -> Value {
    json!({
        "version": VERSION,
        "launch": { "delayMs": 2500, "replacePrevious": true, "randomPages": 5, "restoreWindowRect": false, "startMinimized": false },
        "recents": { "max": 20 },
        // `accounts`: names (comma-separated) to launch when RMI starts on login
        "startup": { "launchOnLogin": false, "minimized": false, "accounts": "" },
        "tray": { "enabled": true },
        // `seen`: last app version whose update notes were shown (null: never)
        // `install`: an update found when RMI opens installs by itself
        "updates": { "auto": true, "install": true, "seen": null },
        "multi": { "enabled": true, "cookieLock": true },
        // Each client starts capped at 60 FPS and graphics level 1 (lightest); both changeable.
        "fps": { "unlock": true, "value": 60 },
        "graphics": { "set": true, "level": 1 },
        "client": { "customSettingsPath": null, "bootstrapperPassthrough": true },
        "presence": { "enabled": true, "intervalSec": 30 },
        "session": { "validateIntervalMin": 30 },
        "images": { "enabled": true },
        "ui": { "showUsernames": true, "density": "comfortable", "scale": 100, "reducedMotion": "system", "tabOrder": ["home", "accounts", "discover", "automation", "devices", "settings"] },
        "vault": { "autoLockMin": 15, "lockOnSleep": true },
        // Opt-in: the default provider is plain HTTP and sees every server IP looked up.
        "geo": { "enabled": false, "urlTemplate": "http://ip-api.com/json/{ip}", "format": "{city}, {countryCode}" },
        "automation": { "enabled": false },
        "ws": { "port": 7810, "startOnLaunch": false },
        "api": { "port": 7811, "enableRead": false, "enableLaunch": false, "enableEdit": false, "enableSocial": false, "enableSecrets": false },
        "net": { "allowLan": false },
        "friend": { "enabled": false, "port": 7812, "publicUrl": "", "username": "friend", "ownerUsername": "", "canAdd": false },
        // Remote access host: Tailscale address only; `deviceName` "" = the computer name.
        "remote": { "enabled": false, "port": 7813, "deviceName": "", "files": false, "audio": true },
        "relaunch": { "spacingSec": 9, "defaultDelaySec": 60, "checkInternet": true, "usePresence": false },
        "clients": { "minimizeEverySec": 0, "closeAfterMin": 0, "closeScope": "instance", "maxInstances": 0 },
        // Local file for the owner's HomeLab agent: account names + running pids, no secrets.
        "homelab": { "export": false },
        "antiAfk": { "enabled": false, "everyMin": 10, "idleSec": 5 },
        "watcher": { "enabled": false, "scanSec": 6, "closeLowMemory": false, "lowMemoryMb": 200, "closeWrongTitle": false, "expectedTitle": "Roblox", "closeNoServer": false, "noServerSec": 60, "ignoreExisting": true, "saveWindowRects": false, "reconnect": false, "waitForGame": false },
        // Grid Arrange default (no settings row). `layout` is replaced whole, never merged
        // (with patches or these defaults): a stale key would make it a different layout.
        "gridArrange": { "monitor": null, "order": [], "layout": { "type": "columns", "count": 3 } },
        "theme": "dark",
        "dev": { "enabled": false },
        "onboarding": { "tosSeen": false, "trayToastSeen": false, "tourDone": false, "tourSeen": [] }
    })
}

const LAYOUT: &str = "/gridArrange/layout";

/// Privacy mode (`ui.showUsernames` off): no account names outside the app's own UI.
pub fn names_hidden(s: &Value) -> bool {
    s.pointer("/ui/showUsernames").and_then(Value::as_bool) == Some(false)
}

/// Roblox window title for `username`: plain "Roblox" in privacy mode.
pub fn window_title(s: &Value, username: &str) -> String {
    if names_hidden(s) { "Roblox" } else { username }.to_owned()
}

const RANGES: &[(&str, i64, i64)] = &[
    ("/launch/delayMs", 0, 60_000),
    ("/launch/randomPages", 1, 50),
    ("/recents/max", 1, 50),
    ("/fps/value", 1, 240),
    ("/graphics/level", 1, 10),
    ("/presence/intervalSec", 15, 300),
    ("/session/validateIntervalMin", 5, 1440),
    ("/ui/scale", 80, 150),
    ("/vault/autoLockMin", 0, 1440),
    ("/ws/port", 1024, 65535),
    ("/api/port", 1024, 65535),
    ("/friend/port", 1024, 65535),
    ("/remote/port", 1024, 65535),
    ("/relaunch/spacingSec", 0, 3600),
    ("/relaunch/defaultDelaySec", 0, 3600),
    ("/clients/minimizeEverySec", 0, 86_400),
    ("/clients/closeAfterMin", 0, 10_080),
    ("/clients/maxInstances", 0, 100),
    ("/antiAfk/everyMin", 1, 19),
    ("/antiAfk/idleSec", 1, 120),
    ("/watcher/scanSec", 2, 600),
    ("/watcher/lowMemoryMb", 20, 1000),
    ("/watcher/noServerSec", 10, 3600),
];

const ENUMS: &[(&str, &[&str])] = &[
    ("/theme", &["system", "dark", "light"]),
    ("/ui/density", &["comfortable", "compact"]),
    ("/ui/reducedMotion", &["system", "on", "off"]),
    ("/clients/closeScope", &["instance", "global"]),
];

/// Ordered upgrades: `MIGRATIONS[n]` takes a version-n document to n+1.
const MIGRATIONS: &[fn(&mut Value)] = &[
    |_v| { /* v0 -> v1: no key changes */ },
    // v1 -> v2: start every client light (60 FPS, graphics 1), as new installs do.
    |v| {
        v["fps"] = json!({ "unlock": true, "value": 60 });
        v["graphics"] = json!({ "set": true, "level": 1 });
    },
];

/// Upgrade an older settings document to `VERSION`.
pub fn migrate(mut v: Value) -> Value {
    if !v.is_object() {
        return defaults();
    }
    let from = v.get("version").and_then(Value::as_u64).unwrap_or(0) as usize;
    if from < MIGRATIONS.len() {
        for m in &MIGRATIONS[from..] {
            m(&mut v);
        }
        v["version"] = json!(VERSION);
    }
    v
}

/// Same-typed user values win over defaults; wrong types fall back; unknown keys are kept.
fn overlay(def: &Value, user: &Value) -> Value {
    match (def, user) {
        (Value::Object(d), Value::Object(u)) => {
            let mut out: Map<String, Value> = u.clone();
            for (k, dv) in d {
                let merged = match u.get(k) {
                    Some(uv) => overlay(dv, uv),
                    None => dv.clone(),
                };
                out.insert(k.clone(), merged);
            }
            Value::Object(out)
        }
        (Value::Null, u) => u.clone(),
        (d, u) if std::mem::discriminant(d) == std::mem::discriminant(u) => u.clone(),
        (d, _) => d.clone(),
    }
}

pub fn normalize(v: &Value) -> Value {
    let def = defaults();
    let user = migrate(v.clone());
    let mut out = overlay(&def, &user);
    if let Some(l) = user.pointer(LAYOUT).filter(|l| l.is_object()) {
        out["gridArrange"]["layout"] = l.clone();
    }
    for (ptr, lo, hi) in RANGES {
        if let Some(x) = out.pointer_mut(ptr) {
            *x = match x.as_i64() {
                Some(n) => json!(n.clamp(*lo, *hi)),
                None => def.pointer(ptr).cloned().unwrap(),
            };
        }
    }
    if let Some(x) = out.pointer_mut("/watcher/expectedTitle") {
        *x = match x.as_str().map(str::trim) {
            Some(s) if !s.is_empty() => json!(s),
            _ => def.pointer("/watcher/expectedTitle").cloned().unwrap(),
        };
    }
    if let Some(x) = out.pointer_mut("/friend/username") {
        *x = match x.as_str().map(str::trim) {
            Some(s) if (1..=32).contains(&s.chars().count()) => json!(s),
            _ => def.pointer("/friend/username").cloned().unwrap(),
        };
    }
    if let Some(x) = out.pointer_mut("/friend/ownerUsername") {
        // empty = off, so only a bad value (too long, not text) falls back to it
        *x = match x.as_str().map(str::trim) {
            Some(s) if s.chars().count() <= 32 => json!(s),
            _ => json!(""),
        };
    }
    if let Some(x) = out.pointer_mut("/remote/deviceName") {
        *x = json!(crate::remote::clean_name(x.as_str().unwrap_or_default()));
    }
    if let Some(x) = out.pointer_mut("/client/customSettingsPath") {
        if !(x.is_null() || x.is_string()) {
            *x = Value::Null;
        }
    }
    for (ptr, allowed) in ENUMS {
        if let Some(x) = out.pointer_mut(ptr) {
            if !x.as_str().is_some_and(|s| allowed.contains(&s)) {
                *x = def.pointer(ptr).cloned().unwrap();
            }
        }
    }
    out
}

/// Reject patch values whose JSON type differs from the default's. Null defaults accept
/// null or string. Unknown paths pass.
fn check_types(def: Option<&Value>, patch: &Value, path: &str) -> Result<(), String> {
    if let (Some(Value::Object(_)), Value::Object(p)) = (def, patch) {
        for (k, pv) in p {
            check_types(def.and_then(|d| d.get(k)), pv, &format!("{path}/{k}"))?;
        }
        return Ok(());
    }
    let ok = match def {
        None => true,
        Some(Value::Null) => patch.is_null() || patch.is_string(),
        Some(d) => std::mem::discriminant(d) == std::mem::discriminant(patch),
    };
    if ok {
        Ok(())
    } else {
        Err(format!("wrong type for setting {path}"))
    }
}

/// Deep-merge `patch` into `cur`, then normalize.
pub fn apply_patch(cur: &Value, patch: &Value) -> Result<Value, String> {
    fn merge(a: &mut Value, b: &Value) {
        match (a, b) {
            (Value::Object(a), Value::Object(b)) => {
                for (k, bv) in b {
                    merge(a.entry(k.clone()).or_insert(Value::Null), bv);
                }
            }
            (a, b) => *a = b.clone(),
        }
    }
    if !cur.is_object() || !patch.is_object() {
        return Err("settings must be a JSON object".into());
    }
    check_types(Some(&defaults()), patch, "")?;
    let mut next = cur.clone();
    if let (Some(l), Some(_)) = (next.pointer_mut(LAYOUT), patch.pointer(LAYOUT)) {
        *l = Value::Null; // replaced whole by the merge below
    }
    merge(&mut next, patch);
    let next = normalize(&next);
    // Only a clash this patch creates counts: one already stored (an upgrade added a
    // default port, or a hand-edited file) must not block unrelated saves.
    let port = |k: &str| next.pointer(&format!("/{k}/port"));
    let set = |k: &str| patch.pointer(&format!("/{k}/port")).is_some();
    let clash = |a: &str, b: &str| port(a).is_some() && port(a) == port(b) && (set(a) || set(b));
    if clash("ws", "api") {
        return Err("The control server and the local API need different ports.".into());
    }
    if clash("friend", "ws") || clash("friend", "api") {
        return Err("The friend server needs its own port.".into());
    }
    if ["ws", "api", "friend"].iter().any(|k| clash("remote", k)) {
        return Err("Remote access needs its own port.".into());
    }
    Ok(next)
}

pub struct LoadOutcome {
    pub value: Value,
    /// Startup message to log (error level) when the file was bad or unreadable.
    pub recovered: Option<String>,
    /// File exists but could not be read: do not let a save clobber it.
    pub read_only: bool,
}

fn outcome(value: Value, recovered: Option<String>, read_only: bool) -> LoadOutcome {
    LoadOutcome {
        value,
        recovered,
        read_only,
    }
}

/// Move a bad file aside to a fresh `settings.json.bad-<ts>` (never overwriting an earlier one).
fn quarantine(path: &Path, why: &str) -> LoadOutcome {
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let mut bad = path.with_extension(format!("json.bad-{ts}"));
    for n in 1.. {
        if !bad.exists() {
            break;
        }
        bad = path.with_extension(format!("json.bad-{ts}-{n}"));
    }
    let moved = std::fs::rename(path, &bad).or_else(|_| std::fs::copy(path, &bad).map(|_| ()));
    match moved {
        Ok(()) => outcome(
            defaults(),
            Some(format!(
                "settings.json is invalid ({why}); kept as {} and using defaults",
                bad.display()
            )),
            false,
        ),
        Err(e) => outcome(
            defaults(),
            Some(format!(
                "settings.json is invalid ({why}) and could not be set aside ({e}); using defaults, saving disabled this session"
            )),
            true,
        ),
    }
}

/// Missing file -> defaults. Invalid file -> set aside, defaults. Unreadable -> defaults, read-only.
pub fn load(path: &Path) -> LoadOutcome {
    match std::fs::read(path) {
        Ok(b) => match serde_json::from_slice::<Value>(&b) {
            Ok(v) if v.is_object() => outcome(normalize(&v), None, false),
            Ok(_) => quarantine(path, "not a JSON object"),
            Err(e) => quarantine(path, &e.to_string()),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => outcome(defaults(), None, false),
        Err(e) => outcome(
            defaults(),
            Some(format!(
                "could not read {} ({e}); using defaults, saving disabled this session",
                path.display()
            )),
            true,
        ),
    }
}

pub fn save(path: &Path, v: &Value) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("json.tmp");
    let write = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&serde_json::to_vec_pretty(v)?)?;
        f.sync_all()?;
        drop(f);
        // Windows AV/indexers briefly lock files; retry the rename.
        let mut r = std::fs::rename(&tmp, path);
        for _ in 0..2 {
            if r.is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
            r = std::fs::rename(&tmp, path);
        }
        r
    };
    let r = write();
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at<'a>(v: &'a Value, p: &str) -> &'a Value {
        v.pointer(p).unwrap()
    }

    /// Every key the settings UI catalog binds must exist in `defaults()`.
    #[test]
    fn ui_catalog_keys_exist() {
        let src = include_str!("../../../src/features/settings/catalog.ts");
        let re = regex::Regex::new(r#"(?:\b[bn]|port)\("([\w.]+)"|path: "([\w.]+)""#).unwrap();
        let def = defaults();
        let mut n = 0;
        for c in re.captures_iter(src) {
            let key = c.get(1).or(c.get(2)).unwrap().as_str();
            let ptr = format!("/{}", key.replace('.', "/"));
            assert!(
                def.pointer(&ptr).is_some(),
                "UI key {key} missing in defaults()"
            );
            n += 1;
        }
        assert!(n > 40, "catalog regex matched only {n} keys");
    }

    #[test]
    fn friend_defaults_and_port_range() {
        let v = normalize(&json!({"friend": {"port": 5}}));
        assert_eq!(at(&v, "/friend/port"), &json!(1024));
        assert_eq!(at(&v, "/friend/enabled"), &json!(false));
        assert_eq!(at(&v, "/friend/username"), &json!("friend"));
        assert_eq!(at(&v, "/friend/canAdd"), &json!(false));
        let v = normalize(&json!({"friend": {"canAdd": "yes"}}));
        assert_eq!(at(&v, "/friend/canAdd"), &json!(false));
        for (given, want) in [
            (" Bob ", "Bob"),
            ("pal", "pal"), // a saved name from before the default changed is kept
            ("  ", "friend"),
            (&*"x".repeat(33), "friend"),
        ] {
            let v = normalize(&json!({"friend": {"username": given}}));
            assert_eq!(at(&v, "/friend/username"), &json!(want));
        }
        for (given, want) in [(" me ", "me"), ("  ", ""), (&*"x".repeat(33), "")] {
            let v = normalize(&json!({"friend": {"ownerUsername": given}}));
            assert_eq!(at(&v, "/friend/ownerUsername"), &json!(want));
        }
        assert_eq!(
            at(&normalize(&json!({})), "/friend/ownerUsername"),
            &json!("")
        );
        assert_eq!(
            at(&normalize(&json!({})), "/launch/startMinimized"),
            &json!(false)
        );
    }

    #[test]
    fn remote_off_by_default_own_port_clean_name() {
        let v = normalize(&json!({}));
        assert_eq!(at(&v, "/remote/enabled"), false);
        assert_eq!(at(&v, "/remote/port"), 7813);
        // file transfer stays off until switched on; sound is allowed once remote access is on
        assert_eq!(at(&v, "/remote/files"), false);
        assert_eq!(at(&v, "/remote/audio"), true);
        let v = normalize(&json!({"remote": {"port": 1, "deviceName": "  Den\u{7} PC  "}}));
        assert_eq!(at(&v, "/remote/port"), 1024);
        assert_eq!(at(&v, "/remote/deviceName"), "Den PC");
        for p in [7810, 7811, 7812] {
            assert!(
                apply_patch(&defaults(), &json!({"remote": {"port": p}})).is_err(),
                "{p}"
            );
        }
        assert!(apply_patch(&defaults(), &json!({"remote": {"enabled": "yes"}})).is_err());
    }

    #[test]
    fn grid_layout_replaced_whole() {
        let v = normalize(&json!({}));
        assert_eq!(at(&v, LAYOUT), &json!({"type": "columns", "count": 3}));
        let v = apply_patch(&v, &json!({"gridArrange": {"layout": {"type": "cascade"}}})).unwrap();
        assert_eq!(at(&v, LAYOUT), &json!({"type": "cascade"}));
        let v = apply_patch(&v, &json!({"gridArrange": {"monitor": "m"}})).unwrap();
        assert_eq!(at(&v, LAYOUT), &json!({"type": "cascade"}));
        assert_eq!(at(&normalize(&v), LAYOUT), &json!({"type": "cascade"}));
    }

    #[test]
    fn v1_moves_to_light_start_once() {
        let old = json!({"version": 1, "fps": {"unlock": true, "value": 240}, "graphics": {"set": false, "level": 9}});
        let v = normalize(&old);
        assert_eq!(at(&v, "/fps"), &json!({"unlock": true, "value": 60}));
        assert_eq!(at(&v, "/graphics"), &json!({"set": true, "level": 1}));
        // a v2 document keeps the user's later choice
        let v = apply_patch(
            &v,
            &json!({"fps": {"value": 144}, "graphics": {"set": false}}),
        )
        .unwrap();
        assert_eq!(at(&normalize(&v), "/fps/value"), 144);
        assert_eq!(at(&normalize(&v), "/graphics/set"), false);
    }

    #[test]
    fn privacy_window_title() {
        let s = json!({"ui": {"showUsernames": false}});
        assert_eq!(window_title(&s, "alt"), "Roblox");
        assert_eq!(window_title(&defaults(), "alt"), "alt");
    }

    #[test]
    fn missing_file_gives_defaults() {
        let d = tempfile::tempdir().unwrap();
        let v = load(&d.path().join("settings.json")).value;
        assert_eq!(at(&v, "/launch/delayMs"), 2500);
        assert_eq!(at(&v, "/fps/value"), 60);
        assert_eq!(at(&v, "/graphics/level"), 1);
        assert_eq!(at(&v, "/ws/port"), 7810);
        assert_eq!(at(&v, "/api/port"), 7811);
        assert_eq!(at(&v, "/vault/autoLockMin"), 15);
        assert_eq!(at(&v, "/theme"), "dark");
        assert_eq!(at(&v, "/multi/enabled"), true);
    }

    #[test]
    fn unknown_keys_survive() {
        let v = normalize(&json!({"version":1,"future":{"x":1},"launch":{"newThing":true}}));
        assert_eq!(at(&v, "/future/x"), 1);
        assert_eq!(at(&v, "/launch/newThing"), true);
        assert_eq!(at(&v, "/launch/delayMs"), 2500);
    }

    #[test]
    fn clamps_ranges_and_enums() {
        let v = normalize(&json!({
            "version":2,"launch":{"delayMs":999999},"fps":{"value":0},"presence":{"intervalSec":1},
            "recents":{"max":500},"theme":"neon"
        }));
        assert_eq!(at(&v, "/launch/delayMs"), 60000);
        assert_eq!(at(&v, "/fps/value"), 1);
        assert_eq!(at(&v, "/presence/intervalSec"), 15);
        assert_eq!(at(&v, "/recents/max"), 50);
        assert_eq!(at(&v, "/theme"), "dark");
    }

    #[test]
    fn watcher_ranges() {
        let v = normalize(&json!({"watcher":{"scanSec":1,"lowMemoryMb":5000,"noServerSec":1}}));
        assert_eq!(at(&v, "/watcher/scanSec"), 2);
        assert_eq!(at(&v, "/watcher/lowMemoryMb"), 1000);
        assert_eq!(at(&v, "/watcher/noServerSec"), 10);
        assert_eq!(at(&v, "/watcher/ignoreExisting"), true);
        let e = normalize(&json!({"watcher":{"expectedTitle":"  "}}));
        assert_eq!(at(&e, "/watcher/expectedTitle"), "Roblox");
        let e = normalize(&json!({"watcher":{"expectedTitle":" My Game "}}));
        assert_eq!(at(&e, "/watcher/expectedTitle"), "My Game");
    }

    #[test]
    fn wrong_type_falls_back() {
        let v = normalize(&json!({"launch":{"delayMs":"fast"},"tray":{"enabled":"yes"}}));
        assert_eq!(at(&v, "/launch/delayMs"), 2500);
        assert_eq!(at(&v, "/tray/enabled"), true);
    }

    #[test]
    fn nullable_accepts_string() {
        let v = normalize(&json!({"client":{"customSettingsPath":"C:/x.json"}}));
        assert_eq!(at(&v, "/client/customSettingsPath"), "C:/x.json");
    }

    #[test]
    fn v0_migrates() {
        let v = normalize(&json!({"launch":{"delayMs":100}}));
        assert_eq!(at(&v, "/version"), VERSION);
        assert_eq!(at(&v, "/launch/delayMs"), 100);
    }

    #[test]
    fn corrupt_file_renamed() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.json");
        std::fs::write(&p, "{not json").unwrap();
        let o = load(&p);
        assert_eq!(at(&o.value, "/launch/delayMs"), 2500);
        assert!(o.recovered.is_some() && !o.read_only);
        assert!(!p.exists());
        // second bad file must not overwrite the first .bad
        std::fs::write(&p, "[1]").unwrap();
        assert!(load(&p).recovered.is_some());
        let bads = std::fs::read_dir(d.path())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("settings.json.bad-")
            })
            .count();
        assert_eq!(bads, 2);
    }

    #[test]
    fn non_object_and_bad_utf8_quarantined() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.json");
        std::fs::write(&p, b"\xff\xfe{").unwrap();
        assert!(load(&p).recovered.is_some());
        std::fs::write(&p, "42").unwrap();
        assert!(load(&p).recovered.is_some());
    }

    #[test]
    fn unreadable_is_read_only() {
        let d = tempfile::tempdir().unwrap();
        // a directory at the settings path: read fails with a non-NotFound error
        let p = d.path().join("settings.json");
        std::fs::create_dir(&p).unwrap();
        let o = load(&p);
        assert!(o.read_only && o.recovered.is_some());
        assert!(p.is_dir());
    }

    #[test]
    fn patch_validation() {
        let d = defaults();
        assert!(apply_patch(&d, &json!([1])).is_err());
        assert!(apply_patch(&json!(1), &json!({})).is_err());
        assert!(apply_patch(&d, &json!({"fps":{"value":"fast"}})).is_err());
        assert!(
            apply_patch(&d, &json!({"ws":{"port":7811}})).is_err(),
            "same port as api"
        );
        for p in [7810, 7811] {
            assert!(
                apply_patch(&d, &json!({"friend":{"port":p}})).is_err(),
                "{p}"
            );
        }
        // A clash already stored (0.2.x ws.port 7812, then friend.port defaulted to 7812)
        // blocks only port edits that keep it, not unrelated saves.
        let stale = normalize(&json!({"ws":{"port":7812}}));
        assert!(apply_patch(&stale, &json!({"theme":"light"})).is_ok());
        assert!(apply_patch(&stale, &json!({"friend":{"port":7812}})).is_err());
        assert!(apply_patch(&stale, &json!({"friend":{"port":7820}})).is_ok());
        assert!(apply_patch(&d, &json!({"fps":"x"})).is_err());
        assert!(apply_patch(&d, &json!({"client":{"customSettingsPath":5}})).is_err());
        assert!(apply_patch(&d, &json!({"client":{"customSettingsPath":"a"}})).is_ok());
        assert!(apply_patch(&d, &json!({"client":{"customSettingsPath":null}})).is_ok());
        assert!(apply_patch(&d, &json!({"brandNew":{"x":[1]}})).is_ok());
    }

    #[test]
    fn custom_path_normalizes() {
        let v = normalize(&json!({"client":{"customSettingsPath":7}}));
        assert!(at(&v, "/client/customSettingsPath").is_null());
    }

    #[test]
    fn migration_stamps_version() {
        assert_eq!(VERSION, MIGRATIONS.len() as u64);
        let v = migrate(json!({"version":0}));
        assert_eq!(v["version"], VERSION);
    }

    #[test]
    fn save_leaves_no_tmp() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.json");
        save(&p, &defaults()).unwrap();
        assert!(!d.path().join("settings.json.tmp").exists());
        // rename onto a directory fails; tmp must be cleaned
        let q = d.path().join("dir");
        std::fs::create_dir(&q).unwrap();
        assert!(save(&q, &defaults()).is_err());
        assert!(!d.path().join("dir.json.tmp").exists());
    }

    #[test]
    fn patch_and_roundtrip() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.json");
        let v = apply_patch(&defaults(), &json!({"fps":{"unlock":true,"value":240}})).unwrap();
        save(&p, &v).unwrap();
        let back = load(&p).value;
        assert_eq!(at(&back, "/fps/unlock"), true);
        assert_eq!(at(&back, "/fps/value"), 240);
        assert_eq!(at(&back, "/fps/value"), at(&v, "/fps/value"));
    }

    #[test]
    fn grid_default_roundtrips() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.json");
        let g = json!({"monitor":"\\\\.\\DISPLAY2","order":["a","b"],"layout":{"type":"focus","main":"a","others":"strip"}});
        let v = apply_patch(&defaults(), &json!({ "gridArrange": g })).unwrap();
        save(&p, &v).unwrap();
        let back = load(&p).value;
        assert_eq!(at(&back, "/gridArrange/order"), &json!(["a", "b"]));
        let l: crate::grid::Layout =
            serde_json::from_value(at(&back, "/gridArrange/layout").clone()).unwrap();
        assert!(matches!(l, crate::grid::Layout::Focus { .. }));
        assert_eq!(at(&back, "/gridArrange/monitor"), "\\\\.\\DISPLAY2");
        let d: crate::grid::Layout =
            serde_json::from_value(at(&defaults(), "/gridArrange/layout").clone()).unwrap();
        assert_eq!(d, crate::grid::Layout::default());
    }

    #[test]
    fn enums_fall_back_per_key() {
        let v = normalize(
            &json!({"ui":{"density":"tiny","reducedMotion":"on"},"clients":{"closeScope":"all"},"theme":"light"}),
        );
        assert_eq!(at(&v, "/ui/density"), "comfortable");
        assert_eq!(at(&v, "/ui/reducedMotion"), "on");
        assert_eq!(at(&v, "/clients/closeScope"), "instance");
        assert_eq!(at(&v, "/theme"), "light");
        let p = apply_patch(&defaults(), &json!({"ui":{"density":"tiny"}})).unwrap();
        assert_eq!(at(&p, "/ui/density"), "comfortable");
    }

    #[test]
    fn more_ranges_clamp() {
        let v = normalize(&json!({
            "ws":{"port":80},"api":{"port":70000},
            "antiAfk":{"everyMin":0,"idleSec":500},
            "clients":{"minimizeEverySec":-1,"closeAfterMin":20000,"maxInstances":101},
            "relaunch":{"spacingSec":9999},
            "ui":{"scale":10},"session":{"validateIntervalMin":1}
        }));
        assert_eq!(at(&v, "/ws/port"), 1024);
        assert_eq!(at(&v, "/api/port"), 65535);
        assert_eq!(at(&v, "/antiAfk/everyMin"), 1);
        assert_eq!(at(&v, "/antiAfk/idleSec"), 120);
        assert_eq!(at(&v, "/clients/minimizeEverySec"), 0);
        assert_eq!(at(&v, "/clients/closeAfterMin"), 10080);
        assert_eq!(at(&v, "/clients/maxInstances"), 100);
        assert_eq!(at(&v, "/relaunch/spacingSec"), 3600);
        assert_eq!(at(&v, "/ui/scale"), 80);
        assert_eq!(at(&v, "/session/validateIntervalMin"), 5);
    }

    #[test]
    fn fractional_number_in_ranged_key_reverts_to_default() {
        let v = normalize(&json!({"launch":{"delayMs":2.5}}));
        assert_eq!(at(&v, "/launch/delayMs"), 2500);
    }

    #[test]
    fn wrong_shape_section_reverts_to_defaults() {
        let v = normalize(&json!({"fps":"fast","launch":[1,2],"tray":null}));
        assert_eq!(at(&v, "/fps"), &json!({"unlock": true, "value": 60}));
        assert_eq!(at(&v, "/launch/delayMs"), 2500);
        assert_eq!(at(&v, "/tray/enabled"), true);
    }

    #[test]
    fn non_object_document_becomes_defaults() {
        assert_eq!(normalize(&json!([1, 2])), defaults());
        assert_eq!(normalize(&json!("x")), defaults());
    }

    #[test]
    fn patch_rejects_null_for_typed_setting() {
        assert!(apply_patch(&defaults(), &json!({"launch":{"delayMs":null}})).is_err());
        assert!(apply_patch(&defaults(), &json!({"theme":null})).is_err());
    }

    #[test]
    fn api_port_cannot_match_ws_port() {
        assert!(apply_patch(&defaults(), &json!({"api":{"port":7810}})).is_err());
    }

    #[test]
    fn newer_document_version_is_not_migrated() {
        let v = normalize(&json!({"version": 9, "fps": {"unlock": true, "value": 240}}));
        assert_eq!(at(&v, "/version"), 9);
        assert_eq!(at(&v, "/fps/value"), 240);
    }

    #[test]
    fn non_object_layout_falls_back_to_default() {
        let v = normalize(&json!({"gridArrange": {"layout": "cascade"}}));
        assert_eq!(at(&v, LAYOUT), &json!({"type": "columns", "count": 3}));
    }
}

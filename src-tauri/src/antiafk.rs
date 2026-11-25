//! Anti-AFK: Roblox kicks after 20 idle minutes. Every `antiAfk.everyMin` minutes, once
//! the user has left keyboard and mouse alone for `antiAfk.idleSec` seconds, each
//! running Roblox gets some input (see `Platform::nudge`). Runs with the vault locked too.

use crate::AppState;
use serde_json::Value;
use std::time::{Duration, Instant};
use tauri::Manager;

fn num(s: &Value, p: &str, d: u64) -> u64 {
    s.pointer(p).and_then(Value::as_u64).unwrap_or(d)
}

/// Due when enabled, the interval passed and the user has been idle long enough.
fn due(s: &Value, last: Option<Instant>, idle_ms: Option<u64>, now: Instant) -> bool {
    s.pointer("/antiAfk/enabled").and_then(Value::as_bool) == Some(true)
        && last.is_none_or(|t| now >= t + Duration::from_secs(num(s, "/antiAfk/everyMin", 10) * 60))
        && idle_ms.is_some_and(|ms| ms >= num(s, "/antiAfk/idleSec", 5) * 1000)
}

pub fn spawn(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        let mut last = None;
        loop {
            std::thread::sleep(Duration::from_secs(2));
            let st = app.state::<AppState>();
            let s = st
                .settings
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            // checked first: idle tracking (input hooks on Windows) only starts once enabled
            if s.pointer("/antiAfk/enabled") != Some(&Value::Bool(true))
                || !due(&s, last, st.platform.idle_ms(), Instant::now())
            {
                continue;
            }
            if st.platform.roblox_running() != Some(true) {
                continue; // try again as soon as something runs
            }
            let n = st.platform.nudge();
            log::info!("anti-AFK: nudged {n} client(s)");
            last = Some(Instant::now()); // even on 0: never focus-swap every 2 s
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn due_needs_enabled_interval_and_idle() {
        let on = json!({"antiAfk": {"enabled": true, "everyMin": 10, "idleSec": 5}});
        let now = Instant::now();
        assert!(due(&on, None, Some(5000), now));
        assert!(!due(&on, None, Some(4999), now), "user active");
        assert!(!due(&on, None, None, now), "idle unknown");
        assert!(!due(
            &json!({"antiAfk": {"enabled": false}}),
            None,
            Some(9e6 as u64),
            now
        ));
        let ago = |m: u64| now.checked_sub(Duration::from_secs(m * 60));
        assert!(!due(&on, ago(9), Some(9000), now), "too soon");
        if let Some(t) = ago(10) {
            assert!(due(&on, Some(t), Some(9000), now));
        }
    }

    #[test]
    fn due_defaults_and_strict_enabled() {
        let now = Instant::now();
        // missing or wrong-typed keys fall back to 10 min / 5 s
        let bare = json!({"antiAfk": {"enabled": true, "everyMin": "5"}});
        assert!(due(&bare, None, Some(5000), now));
        assert!(!due(&bare, None, Some(4999), now));
        if let Some(t) = now.checked_sub(Duration::from_secs(9 * 60)) {
            assert!(!due(&bare, Some(t), Some(9000), now), "default is 10 min");
        }
        // only a real `true` enables
        assert!(!due(
            &json!({"antiAfk": {"enabled": "true"}}),
            None,
            Some(9000),
            now
        ));
    }
}

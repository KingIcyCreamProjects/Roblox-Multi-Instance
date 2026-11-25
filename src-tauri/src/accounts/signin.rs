//! "Sign in with Roblox" (spec §6.2): the official login page in a `login-<uuid>` window
//! with its own throwaway data dir. The label matches no capability, so the page gets no
//! IPC. We never inject JS or read the form; we only poll the window's cookie jar.

use super::cmds::{emit_changed, mutate};
use super::{new_id, signin_upsert, Identity};
use crate::commands::{with_vault, CmdResult, Gen, WAS_LOCKED};
use crate::roblox::{api::users, RobloxError, Secret};
use crate::AppState;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{webview::Cookie, AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use zeroize::Zeroizing;

const LOGIN_URL: &str = "https://www.roblox.com/login";
const COOKIE_URL: &str = "https://www.roblox.com";
const LABEL_PREFIX: &str = "login-";
const DIR_PREFIX: &str = "rmi-login-";
/// Error code: a sign-in window is already open (it gets focus instead).
const BUSY: &str = "SIGNIN_BUSY";

/// Single flight: one sign-in at a time, from every entry point. Held from `start` until
/// the result is emitted; dropping the guard (also on panic or early return) frees it.
static ACTIVE: AtomicBool = AtomicBool::new(false);
struct Flight;
impl Flight {
    fn claim() -> Option<Self> {
        ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Flight)
    }
}
impl Drop for Flight {
    fn drop(&mut self) {
        ACTIVE.store(false, Ordering::Release);
    }
}

pub fn title(hint: Option<&str>) -> String {
    match hint.map(str::trim).filter(|s| !s.is_empty()) {
        Some(u) => format!("Sign in to Roblox — {u}"),
        None => "Sign in to Roblox".into(),
    }
}

pub fn find_cookie(cookies: &[Cookie<'_>]) -> Option<Zeroizing<String>> {
    cookies
        .iter()
        .find(|c| c.name() == ".ROBLOSECURITY" && !c.value().is_empty())
        .map(|c| Zeroizing::new(c.value().to_owned()))
}

/// Top-level navigation stays on https Roblox (login, 2-step, captcha fallback).
pub fn nav_allowed(u: &tauri::Url) -> bool {
    const HOSTS: [&str; 3] = ["roblox.com", "arkoselabs.com", "funcaptcha.com"];
    if u.as_str() == "about:blank" {
        return true;
    }
    u.scheme() == "https"
        && u.host_str().is_some_and(|h| {
            HOSTS
                .iter()
                .any(|d| h == *d || h.ends_with(&format!(".{d}")))
        })
}

/// `window.open` / target=_blank: never a new window; an allowed URL loads in this one.
pub(crate) fn same_window(
    app: &AppHandle,
    label: &str,
    allowed: fn(&tauri::Url) -> bool,
) -> impl Fn(
    tauri::Url,
    tauri::webview::NewWindowFeatures,
) -> tauri::webview::NewWindowResponse<tauri::Wry>
       + Send
       + 'static {
    let (app, label) = (app.clone(), label.to_owned());
    move |url, _| {
        if allowed(&url) {
            let (app, label) = (app.clone(), label.clone());
            // not inside the webview's own callback
            tauri::async_runtime::spawn(async move {
                if let Some(w) = app.get_webview_window(&label) {
                    if let Err(e) = w.navigate(url) {
                        log::warn!("{label}: navigate failed: {e}");
                    }
                }
            });
        }
        tauri::webview::NewWindowResponse::Deny
    }
}

/// Windows holding a Roblox session: sign-in and site-as-account.
pub fn is_session_window(label: &str) -> bool {
    label.starts_with(LABEL_PREFIX) || label.starts_with(crate::tools::site::LABEL_PREFIX)
}

/// Destroy every session window (vault lock). Returns how many.
pub fn close_session_windows(app: &AppHandle) -> usize {
    let mut n = 0;
    for (label, w) in app.webview_windows() {
        if is_session_window(&label) {
            if let Err(e) = w.destroy() {
                log::warn!("closing {label} failed: {e}");
            }
            n += 1;
        }
    }
    n
}

/// Tauri's app cache dir (portable: `<data>/cache/login`); never inside the asset scope
/// (`cache/thumbs`). Temp as a fallback. An isolated debug instance (`RMI_DATA_DIR`) keeps
/// its own, so startup cleanup never sweeps the installed app's sign-in profiles.
pub(crate) fn base_dir(app: &AppHandle) -> PathBuf {
    let st = app.state::<AppState>();
    if st.portable || crate::paths::override_dir().is_some() {
        return st.paths.cache.join("login");
    }
    app.path()
        .app_cache_dir()
        .unwrap_or_else(|_| std::env::temp_dir())
}

pub async fn start(
    app: AppHandle,
    account_id: Option<String>,
    hint: Option<String>,
    password: Option<Zeroizing<String>>,
) -> CmdResult<()> {
    let Some(flight) = Flight::claim() else {
        if let Some((_, w)) = app
            .webview_windows()
            .into_iter()
            .find(|(l, _)| l.starts_with(LABEL_PREFIX))
        {
            let _ = w.unminimize();
            let _ = w.set_focus();
        }
        return Err(BUSY.into());
    };
    let st = app.state::<AppState>();
    let start_gen = Gen::now(&st.lock_gen);
    with_vault(&st.vault, |v| match &account_id {
        Some(id) => super::find(v.payload(), id).map(|_| ()),
        None => Ok(()),
    })?;
    let id = new_id();
    let label = format!("{LABEL_PREFIX}{id}");
    let dir = base_dir(&app).join(format!("{DIR_PREFIX}{id}"));
    WebviewWindowBuilder::new(
        &app,
        &label,
        WebviewUrl::External(LOGIN_URL.parse().expect("valid url")),
    )
    .title(title(hint.as_deref()))
    .inner_size(480.0, 760.0)
    .data_directory(dir.clone())
    .incognito(true)
    .on_new_window(same_window(&app, &label, nav_allowed))
    .on_navigation(|u| {
        let ok = nav_allowed(u);
        if !ok {
            log::info!(
                "sign-in window: blocked navigation to {}",
                u.host_str().unwrap_or("?")
            );
        }
        ok
    })
    .build()
    .map_err(|e| {
        log::warn!("sign-in window failed: {e}");
        "Could not open the sign-in window.".to_string()
    })?;
    tauri::async_runtime::spawn(poll(
        app.clone(),
        label,
        dir,
        hint,
        account_id,
        password,
        start_gen,
        flight,
    ));
    Ok(())
}

#[allow(clippy::too_many_arguments)] // one spawn site; a params struct is not worth it
async fn poll(
    app: AppHandle,
    label: String,
    dir: PathBuf,
    hint: Option<String>,
    account_id: Option<String>,
    password: Option<Zeroizing<String>>,
    start_gen: Gen,
    flight: Flight,
) {
    let url: tauri::Url = COOKIE_URL.parse().expect("valid url");
    // async task, not the main thread: cookies_for_url deadlocks there on Windows
    let r = loop {
        // A lock while the window was being built missed it: close it now (below), before
        // the first wait. A later lock destroys the window, caught just below.
        if !start_gen.current(&app.state::<AppState>().lock_gen) {
            break Err(WAS_LOCKED.into());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        let Some(w) = app.get_webview_window(&label) else {
            // a lock destroys the window too: report that, not "cancelled"
            break Err(if start_gen.current(&app.state::<AppState>().lock_gen) {
                "Sign-in cancelled.".to_string()
            } else {
                WAS_LOCKED.into()
            });
        };
        let found = match w.cookies_for_url(url.clone()) {
            Ok(c) => find_cookie(&c),
            Err(e) => {
                log::warn!("reading sign-in cookies failed: {e}");
                None
            }
        };
        if let Some(c) = found {
            let pw = password.clone();
            match finish(
                &app,
                c,
                hint.as_deref(),
                account_id.as_deref(),
                pw,
                start_gen,
            )
            .await
            {
                // Roblox unreachable: keep the signed-in window and retry, no full re-login
                Ok(None) => {
                    let _ = w.set_title("Sign in to Roblox — can't reach Roblox, retrying…");
                    tokio::time::sleep(Duration::from_secs(4)).await;
                }
                Ok(Some(u)) => break Ok(u),
                Err(e) => break Err(e),
            }
        }
    };
    if let Some(w) = app.get_webview_window(&label) {
        if let Err(e) = w.destroy() {
            log::warn!("closing sign-in window failed: {e}");
        }
    }
    let (ok, username, error) = match r {
        Ok(u) => (true, Some(u), None),
        Err(e) => (false, None, Some(e)),
    };
    if ok {
        log::info!("sign-in finished");
        emit_changed(&app);
        super::sync::kick_headshots(&app);
    }
    if let Err(e) = app.emit(
        "signin://result",
        json!({"ok": ok, "username": username, "error": error}),
    ) {
        log::warn!("could not emit signin://result: {e}");
    }
    drop(flight);
    remove_dir_retry(&dir).await;
}

/// An empty hint matches anyone; otherwise the signed-in username must equal it (case-insensitive).
fn hint_matches(hint: Option<&str>, username: &str) -> bool {
    hint.map(str::trim)
        .filter(|h| !h.is_empty())
        .is_none_or(|h| h.eq_ignore_ascii_case(username))
}

/// `Ok(None)`: a transient Roblox failure; the caller retries with the window still open.
async fn finish(
    app: &AppHandle,
    cookie: Zeroizing<String>,
    hint: Option<&str>,
    account_id: Option<&str>,
    password: Option<Zeroizing<String>>,
    start_gen: Gen,
) -> CmdResult<Option<String>> {
    let st = app.state::<AppState>();
    start_gen.check(&st.lock_gen)?;
    let u = match users::authenticated(&st.roblox, &Secret::new(cookie.as_str())).await {
        Ok(u) => u,
        Err(e) if transient(&e) => {
            log::warn!("sign-in: {e}; retrying");
            return Ok(None);
        }
        Err(e) => return Err(e.to_string()),
    };
    let who = Identity {
        user_id: u.id,
        username: u.name,
        display_name: u.display_name,
    };
    // A password typed for the hinted name is not stored for a different user who signed in.
    let password = password.filter(|_| hint_matches(hint, &who.username));
    mutate(&st, start_gen, |p| {
        signin_upsert(p, account_id, &who, cookie, password)
    })?;
    Ok(Some(who.username))
}

fn transient(e: &RobloxError) -> bool {
    matches!(
        e,
        RobloxError::Network(_)
            | RobloxError::RateLimited
            | RobloxError::Status { code: 500.., .. }
    )
}

/// WebView2 can hold files for a moment after the window closes.
pub(crate) async fn remove_dir_retry(dir: &Path) {
    for _ in 0..20 {
        match std::fs::remove_dir_all(dir) {
            Ok(()) => return,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(_) => tokio::time::sleep(Duration::from_millis(500)).await,
        }
    }
    log::warn!("could not delete {}; removed on next start", dir.display());
}

/// Delete leftover `rmi-login-*` / `rmi-site-*` dirs (best effort; busy ones go next time).
/// A site dir whose window is still open is kept (its Destroyed handler removes it).
pub fn clean_temp_dirs(app: &AppHandle) {
    use crate::tools::site;
    let Ok(rd) = std::fs::read_dir(base_dir(app)) else {
        return;
    };
    for e in rd.flatten() {
        let n = e.file_name();
        let n = n.to_string_lossy();
        let open_site = n.strip_prefix(site::DIR_PREFIX).is_some_and(|id| {
            app.get_webview_window(&format!("{}{id}", site::LABEL_PREFIX))
                .is_some()
        });
        if !open_site && (n.starts_with(DIR_PREFIX) || n.starts_with(site::DIR_PREFIX)) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// Close every `login-*` window and delete stray dirs. Returns windows closed.
pub fn kill_stray(app: &AppHandle) -> usize {
    let mut n = 0;
    for (label, w) in app.webview_windows() {
        if label.starts_with(LABEL_PREFIX) {
            if let Err(e) = w.destroy() {
                log::warn!("closing {label} failed: {e}");
            }
            n += 1;
        }
    }
    clean_temp_dirs(app);
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_flight() {
        let a = Flight::claim().expect("free");
        assert!(Flight::claim().is_none());
        drop(a);
        assert!(Flight::claim().is_some());
    }

    #[test]
    fn transient_errors_retry() {
        assert!(transient(&RobloxError::Network("x".into())));
        assert!(transient(&RobloxError::RateLimited));
        let st = |code| RobloxError::Status {
            code,
            body_snippet: String::new(),
        };
        assert!(transient(&st(503)));
        assert!(!transient(&st(400)));
        assert!(!transient(&RobloxError::Unauthorized));
    }

    #[test]
    fn titles() {
        assert_eq!(title(Some(" alt_1 ")), "Sign in to Roblox — alt_1");
        assert_eq!(title(Some("  ")), "Sign in to Roblox");
        assert_eq!(title(None), "Sign in to Roblox");
    }

    #[test]
    fn password_kept_only_for_the_hinted_user() {
        assert!(hint_matches(None, "Alt_1"));
        assert!(hint_matches(Some("  "), "Alt_1"));
        assert!(hint_matches(Some(" alt_1 "), "Alt_1"));
        assert!(!hint_matches(Some("alt_2"), "Alt_1"));
    }

    #[test]
    fn cookie_extraction() {
        let jar = vec![
            Cookie::new("RBXEventTrackerV2", "x"),
            Cookie::new(".ROBLOSECURITY", ""),
        ];
        assert!(find_cookie(&jar).is_none());
        let jar = vec![
            Cookie::new("GuestData", "g"),
            Cookie::new(".ROBLOSECURITY", "_|WARNING:abc"),
        ];
        assert_eq!(find_cookie(&jar).unwrap().as_str(), "_|WARNING:abc");
    }

    #[test]
    fn labels_match_no_capability() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/capabilities");
        let mut n = 0;
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            if e.path().extension().is_none_or(|x| x != "json") {
                continue;
            }
            n += 1;
            let caps: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(e.path()).unwrap()).unwrap();
            for w in caps["windows"].as_array().into_iter().flatten() {
                let w = w.as_str().unwrap();
                assert!(
                    !w.contains('*')
                        && !w.starts_with(LABEL_PREFIX)
                        && !w.starts_with(crate::tools::site::LABEL_PREFIX),
                    "{w} in {:?}",
                    e.path()
                );
            }
        }
        assert!(n > 0);
    }

    #[test]
    fn session_window_labels() {
        for l in ["login-1", "site-abc"] {
            assert!(is_session_window(l), "{l}");
        }
        for l in ["main", "sites", "xlogin-1", "logins"] {
            assert!(!is_session_window(l), "{l}");
        }
    }

    #[test]
    fn navigation_allowlist() {
        let ok = |s: &str| nav_allowed(&s.parse().unwrap());
        for u in [
            "https://www.roblox.com/login",
            "https://roblox.com/",
            "https://auth.roblox.com/x",
            "https://roblox-api.arkoselabs.com/fc",
            "about:blank",
        ] {
            assert!(ok(u), "{u}");
        }
        for u in [
            "http://www.roblox.com/",
            "https://roblox.com.evil.io/",
            "https://evilroblox.com/",
            "https://example.com/",
            "file:///c:/x",
        ] {
            assert!(!ok(u), "{u}");
        }
    }

    #[test]
    fn server_error_boundary_is_transient() {
        let st = |code| RobloxError::Status {
            code,
            body_snippet: String::new(),
        };
        assert!(!transient(&st(499)));
        assert!(transient(&st(500)));
    }

    #[tokio::test]
    async fn remove_dir_retry_removes_tree_and_tolerates_missing() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("rmi-login-x");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub").join("f"), b"x").unwrap();
        remove_dir_retry(&dir).await;
        assert!(!dir.exists());
        // already gone: returns at once
        remove_dir_retry(&dir).await;
    }
}

//! roblox.com as an account (groups etc.): a `site-<uuid>` window with its own throwaway
//! data dir. The label matches no capability (no IPC). The cookie goes in through the
//! webview's cookie store, never through JS; navigation stays on Roblox.

use super::{acct, still_unlocked};
use crate::accounts::{new_id, signin};
use crate::commands::CmdResult;
use crate::commands::LOCKED;
use crate::AppState;
use tauri::{
    webview::Cookie, AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder, WindowEvent,
};

pub const LABEL_PREFIX: &str = "site-";
pub const DIR_PREFIX: &str = "rmi-site-";

/// Navigation inside the window: Roblox pages, plus the blank start page.
pub fn nav_ok(u: &tauri::Url) -> bool {
    u.as_str() == "about:blank" || url_ok(u)
}

/// Only https `roblox.com` and its subdomains.
pub fn url_ok(u: &tauri::Url) -> bool {
    u.scheme() == "https"
        && u.host_str()
            .is_some_and(|h| h == "roblox.com" || h.ends_with(".roblox.com"))
}

#[tauri::command]
pub async fn account_open_site(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    url: String,
) -> CmdResult<()> {
    let url: tauri::Url = url
        .trim()
        .parse()
        .ok()
        .filter(url_ok)
        .ok_or("Only roblox.com pages can be opened.")?;
    let a = acct(&state, &account_id)?;
    let id = new_id();
    let dir = signin::base_dir(&app).join(format!("{DIR_PREFIX}{id}"));
    let label = format!("{LABEL_PREFIX}{id}");
    let w = WebviewWindowBuilder::new(
        &app,
        &label,
        WebviewUrl::External("about:blank".parse().expect("valid url")),
    )
    .title("Roblox")
    .inner_size(1100.0, 800.0)
    .data_directory(dir.clone())
    .incognito(true)
    .on_new_window(signin::same_window(&app, &label, url_ok))
    .on_navigation(nav_ok)
    .build()
    .map_err(|e| {
        log::warn!("site window failed: {e}");
        "Could not open the window.".to_string()
    })?;
    w.on_window_event(move |e| {
        if matches!(e, WindowEvent::Destroyed) {
            let dir = dir.clone();
            tauri::async_runtime::spawn(async move { signin::remove_dir_retry(&dir).await });
        }
    });
    // Locked while the window was built: the cookie never goes in.
    if !still_unlocked(&state, a.gen) {
        let _ = w.destroy();
        return Err(LOCKED.into());
    }
    // borrowed: no extra copy of the cookie on our side (WebView keeps its own)
    let c = Cookie::build((".ROBLOSECURITY", a.ck.expose()))
        .domain(".roblox.com")
        .path("/")
        .secure(true)
        .http_only(true)
        .build();
    let r = w.set_cookie(c).and_then(|_| w.navigate(url));
    if let Err(e) = r {
        log::warn!("site window setup failed: {e}");
        let _ = w.destroy();
        return Err("Could not open the window.".into());
    }
    // a lock that finished before the window existed could not close it; one that ran
    // meanwhile already did (no second destroy then)
    if !still_unlocked(&state, a.gen) {
        if app.get_webview_window(&label).is_none() {
            return Err(LOCKED.into());
        }
        if let Err(e) = w.destroy() {
            log::warn!("site window: destroy after lock failed: {e}");
        }
        return Err(LOCKED.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_roblox_https() {
        let ok = |s: &str| url_ok(&s.parse().unwrap());
        assert!(ok("https://www.roblox.com/communities/1/x"));
        assert!(ok("https://roblox.com/"));
        assert!(nav_ok(&"about:blank".parse().unwrap()));
        assert!(!nav_ok(
            &"https://roblox-api.arkoselabs.com/fc".parse().unwrap()
        ));
        for u in [
            "http://www.roblox.com/",
            "https://roblox.com.evil.io/",
            "https://evilroblox.com/",
            "https://roblox-api.arkoselabs.com/",
        ] {
            assert!(!ok(u), "{u}");
        }
    }

    #[test]
    fn host_is_parsed_not_matched_as_text() {
        let ok = |s: &str| url_ok(&s.parse().unwrap());
        assert!(!ok("https://www.roblox.com@evil.io/"));
        assert!(!ok("https://roblox.com:x@evil.io/"));
        // a backslash ends the host; case and a trailing dot are normalised or refused
        assert!(!ok("https://evil.io\\.roblox.com/"));
        assert!(ok("https://www.roblox.com\\@evil.io/"));
        assert!(ok("https://ROBLOX.COM/"));
        assert!(!ok("https://roblox.com./"));
        assert!(!"https://roblox.com%2eevil.io/"
            .parse::<tauri::Url>()
            .is_ok_and(|u| url_ok(&u)));
    }
}

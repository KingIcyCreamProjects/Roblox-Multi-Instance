//! Background work (spec §5.4, §6.6): presence polling, session validation, headshot cache.
//! Loops idle while the vault is locked; results are dropped if a lock happened mid-flight.

use super::cmds::{emit_changed, thumb_file};
use super::{map_presence, now_iso, session_of, set_session, SessionState};
use crate::commands::{guard, Gen};
use crate::roblox::{
    api::{games, presence, thumbnails, users},
    Secret,
};
use crate::AppState;
use std::collections::HashSet;
use std::sync::atomic::Ordering::SeqCst;
use std::sync::PoisonError;
use std::time::{Duration, Instant, SystemTime};
use tauri::{AppHandle, Manager};

const THUMB_TTL: Duration = Duration::from_secs(24 * 3600);
const GAME_TTL: Duration = Duration::from_secs(600);

fn setting(app: &AppHandle, ptr: &str) -> Option<serde_json::Value> {
    let st = app.state::<AppState>();
    let s = st.settings.lock().unwrap_or_else(PoisonError::into_inner);
    s.pointer(ptr).cloned()
}

fn unlocked(st: &AppState) -> bool {
    guard(&st.vault).is_some()
}

pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(presence_loop(app.clone()));
    tauri::async_runtime::spawn(validate_loop(app));
}

/// Settings are re-read every 5 s, so turning presence off or shortening the interval
/// applies at once; off also clears the shown presence.
async fn presence_loop(app: AppHandle) {
    let mut last: Option<Instant> = None;
    loop {
        let on = setting(&app, "/presence/enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let secs = setting(&app, "/presence/intervalSec")
            .and_then(|v| v.as_u64())
            .unwrap_or(30)
            .clamp(15, 300);
        if !on {
            last = None;
            let st = app.state::<AppState>();
            let mut map = st
                .accounts
                .presence
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if !map.is_empty() {
                map.clear();
                drop(map);
                emit_changed(&app);
            }
        } else if !unlocked(&app.state::<AppState>()) {
            last = None;
        } else if last.is_none_or(|t| t.elapsed() >= Duration::from_secs(secs)) {
            presence_tick(&app).await;
            last = Some(Instant::now());
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn presence_tick(app: &AppHandle) {
    let st = app.state::<AppState>();
    let start = Gen::now(&st.lock_gen);
    // Authenticated presence shows game info; use the first account with a good session.
    let snap = guard(&st.vault).as_ref().map(|v| {
        let p = v.payload();
        let ids: Vec<(String, u64)> = p
            .accounts
            .iter()
            .filter_map(|a| a.user_id.map(|u| (a.id.clone(), u)))
            .collect();
        let cookie = p
            .accounts
            .iter()
            .filter(|a| a.session == SessionState::Ok)
            .find_map(|a| p.secrets.get(&a.id).filter(|s| !s.cookie.is_empty()))
            .map(|s| Secret::new(s.cookie.as_str()));
        (ids, cookie)
    });
    let Some((ids, cookie)) = snap else { return };
    if ids.is_empty() {
        return;
    }
    let uids: Vec<u64> = ids
        .iter()
        .map(|(_, u)| *u)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let pres = match presence::get(&st.roblox, &uids, cookie.as_ref()).await {
        Ok(p) => p,
        Err(e) => {
            log::debug!("presence: {e}");
            return;
        }
    };
    let names = game_names(&st, pres.iter().filter_map(|p| p.universe_id).collect()).await;
    let now = now_iso();
    let mut changed = false;
    {
        let mut map = st
            .accounts
            .presence
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !start.current(&st.lock_gen) {
            return;
        }
        // entries for removed accounts, or users Roblox no longer reports, go
        let before = map.len();
        map.retain(|id, _| {
            ids.iter()
                .any(|(i, u)| i == id && pres.iter().any(|p| p.user_id == *u))
        });
        changed |= map.len() != before;
        for (id, uid) in &ids {
            let Some(p) = pres.iter().find(|p| p.user_id == *uid) else {
                continue;
            };
            let name = p.universe_id.and_then(|u| names.get(&u).cloned());
            let np = map_presence(p, name, &now);
            changed |= map.get(id).is_none_or(|o| !o.same(&np));
            map.insert(id.clone(), np);
        }
    }
    if changed {
        emit_changed(app);
    }
}

/// universeId → name, cached 10 min. Failures just mean no names this round.
async fn game_names(
    st: &AppState,
    universes: HashSet<u64>,
) -> std::collections::HashMap<u64, String> {
    let need: Vec<u64> = {
        let c = st
            .accounts
            .games
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        universes
            .iter()
            .filter(|u| c.get(u).is_none_or(|(_, t)| t.elapsed() > GAME_TTL))
            .copied()
            .collect()
    };
    if !need.is_empty() {
        match games::by_universe(&st.roblox, &need).await {
            Ok(gs) => {
                let mut c = st
                    .accounts
                    .games
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                for g in gs {
                    c.insert(g.id, (g.name, Instant::now()));
                }
            }
            Err(e) => log::debug!("game names: {e}"),
        }
    }
    let c = st
        .accounts
        .games
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    universes
        .into_iter()
        .filter_map(|u| c.get(&u).map(|(n, _)| (u, n.clone())))
        .collect()
}

/// On every unlock, then every `session.validateIntervalMin`.
async fn validate_loop(app: AppHandle) {
    let mut last_gen = None;
    let mut next = Instant::now();
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let st = app.state::<AppState>();
        let g = Some(Gen::now(&st.lock_gen));
        if !unlocked(&st) {
            continue;
        }
        if g != last_gen || Instant::now() >= next {
            let mins = setting(&app, "/session/validateIntervalMin")
                .and_then(|v| v.as_u64())
                .unwrap_or(30)
                .max(5);
            // only a pass that actually ran resets the schedule (busy or locked: retry in 5 s)
            let ran = match Flight::claim(&app) {
                Some(f) => validate(&app, None, f).await,
                None => false,
            };
            if ran {
                last_gen = g;
                next = Instant::now() + Duration::from_secs(mins * 60);
                headshots(&app).await;
            }
        }
    }
}

/// Single flight for validation: claimed before a pass is started (so a caller knows at once
/// whether it runs) and held by the pass; dropping it frees the slot.
pub(crate) struct Flight(AppHandle);
impl Flight {
    pub(crate) fn claim(app: &AppHandle) -> Option<Self> {
        let busy = app
            .state::<AppState>()
            .accounts
            .validating
            .swap(true, SeqCst);
        (!busy).then(|| Flight(app.clone()))
    }
}
impl Drop for Flight {
    fn drop(&mut self) {
        self.0
            .state::<AppState>()
            .accounts
            .validating
            .store(false, SeqCst);
    }
}

/// Check sessions (all, or `ids`) against Roblox; rate limited by the client.
/// `false` = skipped (locked, or a lock happened mid-pass). Results that couldn't be saved
/// are dropped but the pass still counts.
pub(crate) async fn validate(app: &AppHandle, ids: Option<Vec<String>>, _flight: Flight) -> bool {
    let st = app.state::<AppState>();
    let start = Gen::now(&st.lock_gen);
    let list: Vec<(String, Secret)> = match guard(&st.vault).as_ref() {
        Some(v) => {
            let p = v.payload();
            p.accounts
                .iter()
                .filter(|a| ids.as_ref().is_none_or(|ids| ids.contains(&a.id)))
                .filter_map(|a| {
                    let s = p.secrets.get(&a.id).filter(|s| !s.cookie.is_empty())?;
                    Some((a.id.clone(), Secret::new(s.cookie.as_str())))
                })
                .collect()
        }
        None => return false,
    };
    let mut results = Vec::with_capacity(list.len());
    for (id, c) in list {
        if !start.current(&st.lock_gen) {
            return false; // locked: stop; cookies drop (zeroized) here
        }
        let r = users::authenticated(&st.roblox, &c).await;
        results.push(Checked {
            id,
            cookie: c,
            session: session_of(&r),
            user: r.ok(),
        });
    }
    {
        let mut g = guard(&st.vault);
        let Some(v) = g.as_mut() else { return false };
        if !start.current(&st.lock_gen) {
            return false;
        }
        let before = v.payload().clone();
        if apply_checks(v.payload_mut(), results, &now_iso()) {
            if let Err(e) = v.save() {
                // Memory must match disk: the pass is dropped. Still counts as run, so the
                // next try waits the normal interval (no 5 s retry storm against Roblox).
                *v.payload_mut() = before;
                log::warn!("saving session states failed: {}", crate::commands::verr(e));
            }
        }
    }
    emit_changed(app);
    true
}

pub(crate) struct Checked {
    pub id: String,
    /// The cookie that was checked; a result only applies if it is still the saved one.
    pub cookie: Secret,
    pub session: SessionState,
    pub user: Option<users::AuthUser>,
}

/// Apply results; returns whether anything changed (to save). A check that could not reach
/// Roblox (`Unknown`) is no verdict: it keeps the old state and `lastValidatedAt`. An `Ok`
/// account that has lost its cookie becomes `Expired` (it needs a sign-in).
pub(crate) fn apply_checks(
    p: &mut crate::vault::model::VaultPayload,
    results: Vec<Checked>,
    now: &str,
) -> bool {
    let mut dirty = false;
    let lost: Vec<String> = p
        .accounts
        .iter()
        .filter(|a| a.session == SessionState::Ok)
        .filter(|a| p.secrets.get(&a.id).is_none_or(|s| s.cookie.is_empty()))
        .map(|a| a.id.clone())
        .collect();
    for id in lost {
        dirty |= set_session(p, &id, SessionState::Expired, None);
    }
    for c in results
        .into_iter()
        .filter(|c| c.session != SessionState::Unknown)
    {
        let same = p
            .secrets
            .get(&c.id)
            .is_some_and(|s| s.cookie.as_str() == c.cookie.expose());
        if !same {
            continue; // cookie replaced (re-sign-in, paste) while we were checking
        }
        dirty |= set_session(p, &c.id, c.session, None);
        let Some(a) = p.accounts.iter_mut().find(|a| a.id == c.id) else {
            continue;
        };
        a.last_validated_at = Some(now.to_owned());
        dirty = true; // saved too, or the next unlock forgets the check
        if let Some(u) = c.user {
            if a.username != u.name || a.display_name != u.display_name {
                a.username = u.name;
                a.display_name = u.display_name;
                dirty = true;
            }
        }
    }
    dirty
}

fn fresh(path: &std::path::Path) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .is_ok_and(|t| {
            SystemTime::now()
                .duration_since(t)
                .is_ok_and(|d| d < THUMB_TTL)
        })
}

fn images_on(app: &AppHandle) -> bool {
    setting(app, "/images/enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(true)
}

async fn save_image(st: &AppState, url: &str, dest: &std::path::Path) -> bool {
    match st.roblox.get_cdn(url).await {
        Ok(b) => {
            let r = dest
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|_| {
                    // own temp name: concurrent fetches of one image never share a .tmp
                    let tmp = dest.with_extension(format!("{}.tmp", super::new_id()));
                    let r = std::fs::write(&tmp, &b).and_then(|_| std::fs::rename(&tmp, dest));
                    if r.is_err() {
                        let _ = std::fs::remove_file(&tmp);
                    }
                    r
                });
            r.inspect_err(|e| log::warn!("thumbnail write failed: {e}"))
                .is_ok()
        }
        Err(e) => {
            log::debug!("thumbnail download: {e}");
            false
        }
    }
}

/// Fetch missing or stale 48 px headshots for every account.
pub async fn headshots(app: &AppHandle) {
    if !images_on(app) {
        return;
    }
    let st = app.state::<AppState>();
    let need: Vec<u64> = match guard(&st.vault).as_ref() {
        Some(v) => v
            .payload()
            .accounts
            .iter()
            .filter_map(|a| a.user_id)
            .filter(|u| !fresh(&thumb_file(&st, *u, 48)))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect(),
        None => return,
    };
    if need.is_empty() {
        return;
    }
    let thumbs = match thumbnails::headshots(&st.roblox, &need, "48x48").await {
        Ok(t) => t,
        Err(e) => {
            log::debug!("headshots: {e}");
            return;
        }
    };
    let mut any = false;
    for t in thumbs {
        if let (true, Some(url)) = (t.state == "Completed", t.image_url.as_deref()) {
            any |= save_image(&st, url, &thumb_file(&st, t.target_id, 48)).await;
        }
    }
    if any {
        emit_changed(app);
    }
}

pub fn kick_headshots(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move { headshots(&app).await });
}

/// Cached 420x420 full-body PNG path, fetched when missing or stale.
pub async fn avatar_full(app: &AppHandle, user_id: u64) -> Option<String> {
    let st = app.state::<AppState>();
    let f = thumb_file(&st, user_id, 420);
    if !fresh(&f) && images_on(app) {
        if let Ok(Some(t)) = thumbnails::avatar(&st.roblox, user_id, "420x420").await {
            if let (true, Some(url)) = (t.state == "Completed", t.image_url.as_deref()) {
                save_image(&st, url, &f).await;
            }
        }
    }
    f.is_file().then(|| f.display().to_string())
}

#[cfg(test)]
mod tests {
    use crate::accounts::{session_of, SessionState};
    use crate::roblox::{api::users, BaseUrls, RobloxClient, Secret};
    use axum::{http::HeaderMap, http::StatusCode, response::IntoResponse, routing::get, Router};

    /// Mock users API: cookie "good" → 200, anything else → 401.
    async fn serve() -> RobloxClient {
        async fn me(h: HeaderMap) -> axum::response::Response {
            let c = h.get("cookie").and_then(|v| v.to_str().ok()).unwrap_or("");
            if c == ".ROBLOSECURITY=good" {
                axum::Json(serde_json::json!({"id": 1, "name": "a", "displayName": "A"}))
                    .into_response()
            } else {
                StatusCode::UNAUTHORIZED.into_response()
            }
        }
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let app = Router::new().route("/v1/users/authenticated", get(me));
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        RobloxClient::new(BaseUrls::all(&url))
    }

    #[test]
    fn stale_results_ignored() {
        use super::{apply_checks, Checked};
        use crate::accounts::{upsert, Identity};
        use zeroize::Zeroizing;
        let mut p = crate::vault::model::VaultPayload::default();
        let who = |n: &str| Identity {
            user_id: 1,
            username: n.into(),
            display_name: n.into(),
        };
        let (_, id) = upsert(&mut p, &who("a"), Some(Zeroizing::new("old".into())), None);
        let check = |c: &str| Checked {
            id: id.clone(),
            cookie: Secret::new(c),
            session: SessionState::Expired,
            user: None,
        };
        // cookie replaced since the snapshot: no change
        assert!(!apply_checks(&mut p, vec![check("older")], "t"));
        assert_eq!(p.accounts[0].session, SessionState::Ok);
        assert!(apply_checks(&mut p, vec![check("old")], "t"));
        assert_eq!(p.accounts[0].session, SessionState::Expired);
        assert_eq!(p.accounts[0].last_validated_at.as_deref(), Some("t"));
        // offline: no verdict, nothing changes
        let offline = Checked {
            session: SessionState::Unknown,
            ..check("old")
        };
        assert!(!apply_checks(&mut p, vec![offline], "t2"));
        assert_eq!(p.accounts[0].session, SessionState::Expired);
        assert_eq!(p.accounts[0].last_validated_at.as_deref(), Some("t"));
        // ok without a cookie: needs a sign-in
        p.accounts[0].session = SessionState::Ok;
        p.secrets.clear();
        assert!(apply_checks(&mut p, vec![], "t3"));
        assert_eq!(p.accounts[0].session, SessionState::Expired);
    }

    #[tokio::test]
    async fn session_states_from_server() {
        let c = serve().await;
        let ok = users::authenticated(&c, &Secret::new("good")).await;
        assert_eq!(session_of(&ok), SessionState::Ok);
        let bad = users::authenticated(&c, &Secret::new("stale")).await;
        assert_eq!(session_of(&bad), SessionState::Expired);
        let down = RobloxClient::new(BaseUrls::all("http://127.0.0.1:9"));
        let r = users::authenticated(&down, &Secret::new("good")).await;
        assert_eq!(session_of(&r), SessionState::Unknown);
    }

    #[test]
    fn check_takes_new_name_and_stamps_time() {
        use super::{apply_checks, Checked};
        use crate::accounts::{upsert, Identity};
        use zeroize::Zeroizing;
        let mut p = crate::vault::model::VaultPayload::default();
        let who = Identity {
            user_id: 1,
            username: "old".into(),
            display_name: "Old".into(),
        };
        let (_, id) = upsert(&mut p, &who, Some(Zeroizing::new("c".into())), None);
        let renamed = Checked {
            id: id.clone(),
            cookie: Secret::new("c"),
            session: SessionState::Ok,
            user: Some(users::AuthUser {
                id: 1,
                name: "new".into(),
                display_name: "New".into(),
            }),
        };
        assert!(apply_checks(&mut p, vec![renamed], "t"));
        let a = &p.accounts[0];
        assert_eq!(
            (a.username.as_str(), a.display_name.as_str()),
            ("new", "New")
        );
        assert_eq!(a.last_validated_at.as_deref(), Some("t"));
    }

    #[test]
    fn fresh_needs_recent_existing_file() {
        use super::fresh;
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("48.png");
        assert!(!fresh(&f), "missing");
        std::fs::write(&f, b"x").unwrap();
        assert!(fresh(&f), "just written");
    }

    #[test]
    fn cookie_sweep_leaves_pending_alone() {
        use super::apply_checks;
        use crate::accounts::{upsert, Identity};
        use zeroize::Zeroizing;
        let mut p = crate::vault::model::VaultPayload::default();
        let who = Identity {
            user_id: 2,
            username: "p".into(),
            display_name: "P".into(),
        };
        upsert(&mut p, &who, None, Some(Zeroizing::new("pw".into())));
        assert!(!apply_checks(&mut p, vec![], "t"));
        assert_eq!(p.accounts[0].session, SessionState::Pending);
    }
}

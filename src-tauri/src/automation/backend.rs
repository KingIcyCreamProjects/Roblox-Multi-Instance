//! The real API backend (spec §9.3) over the vault accounts and the launcher.

use super::{AddGate, ApiAccount, ApiResult, Backend, LaunchReq, Patch};
use crate::accounts::{self, cmds::mutate, Account, FriendUpsertError, Identity, PresenceState};
use crate::automation::protocol::ApiError;
use crate::commands::{guard, Gen, LOCKED, WAS_LOCKED};
use crate::friend::{self, FriendAccount};
use crate::launch::launcher::{batch_status, start_batch, Host};
use crate::launch::resolve::{Bar, Mode};
use crate::roblox::{
    api::{account, quicklogin, users},
    RobloxError, Secret,
};
use crate::AppState;
use serde_json::{json, Map, Value};
use std::sync::OnceLock;
use zeroize::Zeroizing;

/// The host is set once the Tauri app exists (`set_host` in setup).
pub struct AppBackend<H: Host> {
    host: OnceLock<H>,
}

impl<H: Host> Default for AppBackend<H> {
    fn default() -> Self {
        Self {
            host: OnceLock::new(),
        }
    }
}

impl<H: Host> AppBackend<H> {
    pub fn set_host(&self, h: H) {
        let _ = self.host.set(h);
    }
    fn h(&self) -> ApiResult<&H> {
        self.host
            .get()
            .ok_or_else(|| ApiError::new(503, "starting", "The app is still starting"))
    }
}

fn locked() -> ApiError {
    ApiError::new(423, "vault_locked", "Unlock the vault first")
}

/// Map an IPC error string to an API error.
fn cmd_err(e: String) -> ApiError {
    if e == LOCKED || e == WAS_LOCKED {
        locked()
    } else {
        ApiError::new(400, "bad_request", e)
    }
}

pub fn api_account(a: &Account) -> ApiAccount {
    ApiAccount {
        user_id: a.user_id,
        username: a.username.clone(),
        display_name: a.display_name.clone(),
        alias: a.alias.clone(),
        note: a.note.clone(),
        group: a.group.clone(),
        session: serde_json::to_value(a.session)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default(),
        extra: a.extra.clone(),
    }
}

/// Launch request → launcher bar. `false` = nothing given: use the account's own target.
pub fn bar_of(r: &LaunchReq) -> (Bar, bool) {
    let place = r.place_id.map(|p| p.to_string()).unwrap_or_default();
    let bar = if let Some(f) = r.follow.as_deref().filter(|s| !s.trim().is_empty()) {
        Bar {
            destination: format!("@{}", f.trim().trim_start_matches('@')),
            ..Default::default()
        }
    } else if let Some(l) = r.private_link.as_deref().filter(|s| !s.trim().is_empty()) {
        Bar {
            destination: l.trim().into(),
            ..Default::default()
        }
    } else if let Some(j) = r.job_id.as_deref().filter(|s| !s.trim().is_empty()) {
        Bar {
            destination: place,
            server: j.trim().into(),
            mode: Mode::Specific,
            ..Default::default()
        }
    } else {
        Bar {
            destination: place,
            ..Default::default()
        }
    };
    let given = !bar.destination.is_empty();
    (bar, given)
}

/// Read the account named `name` (username, case-insensitive) from the unlocked vault.
pub fn with_account<T>(
    st: &AppState,
    name: &str,
    f: impl FnOnce(&Account, Option<&accounts::AccountSecrets>) -> T,
) -> ApiResult<T> {
    let g = guard(&st.vault);
    let p = g.as_ref().ok_or_else(locked)?.payload();
    let a = p
        .accounts
        .iter()
        .find(|a| a.username.eq_ignore_ascii_case(name.trim()))
        .ok_or_else(|| super::no_account(name))?;
    Ok(f(a, p.secrets.get(&a.id)))
}

#[async_trait::async_trait]
impl<H: Host> Backend for AppBackend<H> {
    async fn list_accounts(&self) -> ApiResult<Vec<ApiAccount>> {
        let st = self.h()?.st();
        let g = guard(&st.vault);
        let p = g.as_ref().ok_or_else(locked)?.payload();
        Ok(p.accounts.iter().map(api_account).collect())
    }

    async fn get_account(&self, name: &str) -> ApiResult<ApiAccount> {
        with_account(self.h()?.st(), name, |a, _| api_account(a))
    }

    async fn presence(&self, name: &str) -> ApiResult<Value> {
        let st = self.h()?.st();
        let id = with_account(st, name, |a, _| a.id.clone())?;
        let p = st
            .accounts
            .presence
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&id)
            .cloned();
        Ok(json!({ "presence": p }))
    }

    async fn launch(&self, name: &str, req: LaunchReq) -> ApiResult<String> {
        let h = self.h()?;
        let (id, paused) = with_account(h.st(), name, |a, _| (a.id.clone(), a.friend_paused))?;
        // start_batch would clear the pause: an external tool must not undo a friend Disconnect.
        if paused {
            return Err(ApiError::new(
                409,
                "friend_paused",
                "Your friend disconnected this account. Reconnect it first.",
            ));
        }
        let (bar, given) = bar_of(&req);
        // Without a place the jobId would be dropped for the saved launch target.
        if !given && req.job_id.as_deref().is_some_and(|j| !j.trim().is_empty()) {
            return Err(ApiError::new(400, "bad_request", "jobId needs placeId"));
        }
        start_batch(h, vec![id], bar, given, false).map_err(cmd_err)
    }

    async fn launch_status(&self, id: &str) -> ApiResult<Value> {
        batch_status(id).ok_or_else(|| ApiError::not_found(format!("No launch {id}")))
    }

    async fn patch_account(&self, name: &str, patch: Patch) -> ApiResult<ApiAccount> {
        let h = self.h()?;
        let st = h.st();
        let id = with_account(st, name, |a, _| a.id.clone())?;
        let mut m = Map::new();
        let mut put = |k: &str, v: Option<Value>| {
            if let Some(v) = v {
                m.insert(k.into(), v);
            }
        };
        // Some(None) (JSON null) clears: Value::from(None) is Null.
        put("alias", patch.alias.map(Value::from));
        put("note", patch.note.map(Value::from));
        put("group", patch.group.map(Value::from));
        put("extra", patch.extra.map(|e| json!(e)));
        let start = Gen::now(&st.lock_gen);
        let a = blocking(h, move |st| {
            mutate(st, start, |p| {
                accounts::update(p, &id, &Value::Object(m))?;
                Ok(api_account(&p.accounts[accounts::find(p, &id)?]))
            })
        })
        .await?;
        h.emit_json("accounts://changed", Value::Null);
        Ok(a)
    }

    async fn import_cookie(&self, cookie: &str) -> ApiResult<ApiAccount> {
        let h = self.h()?;
        let st = h.st();
        let start = Gen::now(&st.lock_gen);
        if guard(&st.vault).is_none() {
            return Err(locked());
        }
        let c = accounts::normalize_cookie(cookie)
            .map(Zeroizing::new)
            .ok_or_else(|| {
                ApiError::new(400, "bad_request", "That is not a Roblox session cookie")
            })?;
        let u = match users::authenticated(&st.roblox, &Secret::new(c.as_str())).await {
            Ok(u) => u,
            Err(RobloxError::Unauthorized) => {
                return Err(ApiError::new(
                    400,
                    "bad_request",
                    "Roblox says this cookie is signed out or expired",
                ))
            }
            Err(e) => return Err(ApiError::new(502, "roblox_error", e.to_string())),
        };
        let who = Identity {
            user_id: u.id,
            username: u.name,
            display_name: u.display_name,
        };
        let a = blocking(h, move |st| {
            mutate(st, start, |p| {
                let (_, id) = accounts::upsert(p, &who, Some(c), None);
                Ok(api_account(&p.accounts[accounts::find(p, &id)?]))
            })
        })
        .await?;
        h.emit_json("accounts://changed", Value::Null);
        Ok(a)
    }

    async fn block(&self, name: &str, username: &str) -> ApiResult<()> {
        self.set_block(name, username, true).await
    }
    async fn unblock(&self, name: &str, username: &str) -> ApiResult<()> {
        self.set_block(name, username, false).await
    }
    async fn blocked(&self, name: &str) -> ApiResult<Value> {
        let h = self.h()?;
        let st = h.st();
        let (aid, ck) = session(st, name)?;
        let (ids, more) = match account::blocked_ids(&st.roblox, &ck).await {
            Ok(r) => r,
            Err(e) => return Err(session_err(h, aid, ck, e).await),
        };
        let us = users::by_ids(&st.roblox, &ids).await.map_err(rbx_err)?;
        let list: Vec<Value> = us
            .into_iter()
            .map(|u| json!({ "userId": u.id, "username": u.name, "displayName": u.display_name }))
            .collect();
        Ok(json!({ "blocked": list, "truncated": more }))
    }

    async fn cookie(&self, name: &str) -> ApiResult<String> {
        with_account(self.h()?.st(), name, |_, s| {
            s.filter(|s| !s.cookie.is_empty())
                .map(|s| s.cookie.to_string())
        })?
        .ok_or_else(|| ApiError::not_found(format!("{name} has no cookie")))
    }

    async fn friend_list(&self, owner: bool) -> ApiResult<Vec<FriendAccount>> {
        let h = self.h()?.clone();
        // tracked_pids walks the process list: off the async workers.
        tokio::task::spawn_blocking(move || {
            let st = h.st();
            let accs: Vec<_> = {
                let g = guard(&st.vault);
                let p = g.as_ref().ok_or_else(friend::locked)?.payload();
                p.accounts
                    .iter()
                    .filter(|a| owner || a.friend_allowed)
                    .map(|a| {
                        (
                            a.id.clone(),
                            a.display_name.clone(),
                            a.username.clone(),
                            a.tracker_id.clone(),
                            a.friend_paused,
                            a.user_id,
                        )
                    })
                    .collect()
            };
            let pres = st
                .accounts
                .presence
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            let now = chrono::Utc::now().timestamp_millis();
            // Presence not refreshed for three rounds (90 s at least) is not trusted: a failed
            // refresh keeps the old entry, and it would read as "in a game" for ever.
            let stale_ms = {
                let s = st
                    .settings
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let every = s.pointer("/presence/intervalSec").and_then(|v| v.as_i64());
                every.unwrap_or(30).clamp(30, 300) * 3000
            };
            let here = st.remote.device_name();
            // What paired computers say they run: (Roblox user id, computer name).
            let peers = st.remote.running_elsewhere();
            // One process scan for all accounts (polled every few seconds per viewer).
            let trackers: Vec<String> = accs.iter().map(|a| a.3.clone()).collect();
            let live = st.platform.tracked_clients(&trackers);
            Ok(accs
                .into_iter()
                .map(|(id, display_name, username, tracker, paused, uid)| {
                    // An entry without a readable time is kept: nothing says it is old.
                    let p = pres.get(&id).filter(|p| {
                        chrono::DateTime::parse_from_rfc3339(&p.updated_at)
                            .map_or(true, |t| now - t.timestamp_millis() < stale_ms)
                    });
                    let in_game = p.is_some_and(|p| p.state == PresenceState::InGame);
                    let pids: Vec<u32> = live
                        .iter()
                        .filter(|(_, t)| *t == tracker)
                        .map(|(pid, _)| *pid)
                        .collect();
                    // A live client (or an unpaused RMI launch, own grace included) always
                    // offers Disconnect. Paused ignores own_busy: its grace outlives the close.
                    let running = !pids.is_empty() || (!paused && st.automation.own_busy(&id));
                    // Live means RMI has a client (all a Disconnect can close): presence alone
                    // lags, or is the account on another device.
                    let status = match (running, paused) {
                        (false, true) => "paused",
                        (false, false) => "offline",
                        // A launch queued or running and no window yet (not the grace after
                        // one). Before presence: "in a game" may be another computer's client.
                        _ if pids.is_empty() && st.automation.own_running(&id) => "starting",
                        _ if in_game => "inGame",
                        _ => "online",
                    };
                    // A paired computer says it has a client for this account: certain, also
                    // while paused here. Presence alone counts only when not paused (right
                    // after a Disconnect here Roblox still reports the game for a while).
                    let peer = uid
                        .filter(|_| !running)
                        .and_then(|u| peers.iter().find(|(id, _)| *id == u))
                        .map(|(_, name)| name.clone());
                    let elsewhere = peer.is_some() || (in_game && !running && !paused);
                    let started = pids.into_iter().filter_map(crate::watcher::start_ms).min();
                    FriendAccount {
                        id,
                        display_name,
                        username,
                        status,
                        // also for a client on another computer: the page says where.
                        // Not for an account disconnected here and not seen elsewhere.
                        game: p
                            .filter(|_| in_game && (running || elsewhere))
                            .and_then(|p| p.game_name.clone()),
                        uptime_sec: started.map(|s| (now - s).max(0) as u64 / 1000),
                        elsewhere,
                        // Computer names are for the owner's own login only.
                        computer: match (owner, running) {
                            (false, _) => None,
                            (true, true) => Some(here.clone()),
                            (true, false) => peer,
                        },
                    }
                })
                .collect())
        })
        .await
        .map_err(friend::internal)?
    }

    async fn friend_disconnect(&self, id: &str, owner: bool) -> ApiResult<()> {
        let h = self.h()?;
        let st = h.st();
        let (tracker, username, display, was_paused) = friend_account(st, id, owner, |_, a| {
            (
                a.tracker_id.clone(),
                a.username.clone(),
                a.display_name.clone(),
                a.friend_paused,
            )
        })?;
        let place = st
            .accounts
            .presence
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
            .and_then(|p| p.place_id);
        let (id, start) = (id.to_owned(), Gen::now(&st.lock_gen));
        log::info!("friend: disconnect {username}");
        // Pause saved first: auto-relaunch must not bring back what the close ends.
        // Already paused: no vault save, just close again.
        let closed = blocking(h, move |st| {
            if !was_paused {
                mutate(st, start, |p| {
                    let i = accounts::find(p, &id)?;
                    let a = &mut p.accounts[i];
                    a.friend_paused = true;
                    a.friend_last_place = place.or(a.friend_last_place);
                    Ok(())
                })?;
            }
            Ok(st.platform.close_tracked(&tracker))
        })
        .await
        .map_err(friend::lock_err)?;
        friend_done(h, "disconnect", &display);
        closed.map(|_| ()).map_err(|e| {
            log::warn!("friend: close failed for {username}: {e}");
            ApiError::new(500, "close_failed", "Paused, but the game did not close")
        })
    }

    async fn friend_reconnect(
        &self,
        id: &str,
        link: Option<String>,
        owner: bool,
    ) -> ApiResult<String> {
        let h = self.h()?;
        let st = h.st();
        let (has_target, last, username, display, paused) =
            friend_account(st, id, owner, |p, a| {
                let t = accounts::target_for(p, a).is_some();
                (
                    t,
                    a.friend_last_place,
                    a.username.clone(),
                    a.display_name.clone(),
                    a.friend_paused,
                )
            })?;
        // A queued/running launch (or its grace) refuses another; paused skips only the grace,
        // as in friend_list. start_batch counts the launch before its pause save.
        if st.automation.own_running(id) || (!paused && st.automation.own_busy(id)) {
            return Err(ApiError::new(409, "busy", "Already reconnecting"));
        }
        let (bar, all) = friend::reconnect_bar(link, has_target, last)?;
        log::info!("friend: reconnect {username}");
        // start_batch clears the pause (a vault save): off the async workers.
        let (h2, id2) = (h.clone(), id.to_owned());
        let batch =
            tokio::task::spawn_blocking(move || start_batch(&h2, vec![id2], bar, all, true))
                .await
                .map_err(friend::internal)?
                .map_err(cmd_err)
                .map_err(friend::launch_err)?;
        friend_done(h, "reconnect", &display);
        Ok(batch)
    }

    async fn friend_avatar(&self, id: &str, owner: bool) -> ApiResult<Vec<u8>> {
        let st = self.h()?.st();
        let uid = friend_account(st, id, owner, |_, a| a.user_id)?.ok_or_else(friend::not_found)?;
        tokio::fs::read(accounts::cmds::thumb_file(st, uid, 48))
            .await
            .map_err(|_| friend::not_found())
    }

    async fn friend_added(&self) -> ApiResult<usize> {
        let st = self.h()?.st();
        let g = guard(&st.vault);
        let p = g.as_ref().ok_or_else(friend::locked)?.payload();
        Ok(p.accounts.iter().filter(|a| a.friend_added).count())
    }

    async fn friend_add_create(&self) -> ApiResult<quicklogin::Created> {
        quicklogin::create(&self.h()?.st().roblox)
            .await
            .map_err(friend::roblox)
    }

    async fn friend_add_poll(&self, code: &str, key: &Secret) -> ApiResult<quicklogin::Status> {
        quicklogin::status(&self.h()?.st().roblox, code, key)
            .await
            .map_err(friend::roblox)
    }

    async fn friend_add_cancel(&self, code: &str) -> ApiResult<()> {
        quicklogin::cancel(&self.h()?.st().roblox, code)
            .await
            .map_err(friend::roblox)
    }

    async fn friend_add_finish(&self, code: &str, key: &Secret, ok: AddGate) -> ApiResult<String> {
        let h = self.h()?;
        let st = h.st();
        let start = Gen::now(&st.lock_gen);
        if guard(&st.vault).is_none() {
            return Err(friend::locked());
        }
        if !ok() {
            return Err(friend::add_off());
        }
        let ck = quicklogin::login(&st.roblox, code, key)
            .await
            .map_err(friend::roblox)?;
        // A new Roblox session exists now: unless it was stored, sign it out again.
        let r = friend_store(h, start, &ck, ok).await;
        if r.is_err() {
            if let Err(e) = quicklogin::logout(&st.roblox, &ck).await {
                log::debug!("friend: unused session not signed out: {e}");
            }
        }
        r
    }
}

/// Who `ck` is, then add (or refresh) it in the vault if `ok` still holds under the vault lock.
async fn friend_store<H: Host>(h: &H, start: Gen, ck: &Secret, ok: AddGate) -> ApiResult<String> {
    let u = users::authenticated(&h.st().roblox, ck)
        .await
        .map_err(friend::roblox)?;
    let display = u.display_name.clone();
    let who = Identity {
        user_id: u.id,
        username: u.name,
        display_name: u.display_name,
    };
    let c = Zeroizing::new(ck.expose().to_owned());
    log::info!("friend: add {}", who.username);
    blocking(h, move |st| {
        let mut refused = None;
        let r = mutate(st, start, |p| {
            // Under the vault lock (then the settings lock inside `ok`): a gate change
            // during sign-in stops the write.
            if !ok() {
                refused = Some(friend::add_off());
                return Err(String::new());
            }
            accounts::friend_upsert(p, &who, c, friend::MAX_ADDED).map_err(|e| match e {
                FriendUpsertError::Refused(m) => {
                    refused = Some(ApiError::new(409, "add_refused", m));
                    String::new()
                }
                FriendUpsertError::Internal(m) => m,
            })
        });
        match refused {
            Some(e) => Ok(Err(e)),
            None => r.map(Ok),
        }
    })
    .await
    // Locks stay "locked"; any other vault error is the generic one (real text logged).
    .map_err(|e| match e.status {
        423 => friend::locked(),
        500 => e,
        _ => friend::internal(e.message),
    })??;
    h.emit_json("accounts://changed", Value::Null);
    h.kick_headshots();
    friend_done(h, "add", &display);
    Ok(display)
}

/// The friend-allowed account `id`; any other id is the same 404 (no probing which exist).
/// `owner` (the owner's own login): any account.
fn friend_account<T>(
    st: &AppState,
    id: &str,
    owner: bool,
    f: impl FnOnce(&crate::vault::model::VaultPayload, &Account) -> T,
) -> ApiResult<T> {
    let g = guard(&st.vault);
    let p = g.as_ref().ok_or_else(friend::locked)?.payload();
    let a = p
        .accounts
        .iter()
        .find(|a| a.id == id && (owner || a.friend_allowed))
        .ok_or_else(friend::not_found)?;
    Ok(f(p, a))
}

fn friend_done<H: Host>(h: &H, action: &str, display: &str) {
    h.emit_json("state://changed", json!(["accounts"]));
    h.emit_json(
        "friend_action",
        json!({ "action": action, "displayName": display }),
    );
}

/// Vault save (encrypt + fsync) off the async workers.
async fn blocking<H: Host, T: Send + 'static>(
    h: &H,
    f: impl FnOnce(&AppState) -> crate::commands::CmdResult<T> + Send + 'static,
) -> ApiResult<T> {
    let h = h.clone();
    tokio::task::spawn_blocking(move || f(h.st()))
        .await
        .map_err(friend::internal)?
        .map_err(cmd_err)
}

impl<H: Host> AppBackend<H> {
    /// Social family: same Roblox calls as the account tools (M6).
    async fn set_block(&self, name: &str, username: &str, on: bool) -> ApiResult<()> {
        let h = self.h()?;
        let st = h.st();
        let (aid, ck) = session(st, name)?;
        let target = username.trim().trim_start_matches('@');
        if target.is_empty() {
            return Err(ApiError::new(400, "bad_request", "Give a username"));
        }
        let id = users::by_usernames(&st.roblox, &[target.to_owned()])
            .await
            .map_err(rbx_err)?
            .first()
            .map(|u| u.id)
            .ok_or_else(|| ApiError::not_found(format!("No Roblox user is named {target}")))?;
        match account::block(&st.roblox, &ck, id, on).await {
            Ok(()) => Ok(()),
            Err(e) => Err(session_err(h, aid, ck, e).await),
        }
    }
}

/// The account's id and session cookie, or 404 when it has none.
fn session(st: &AppState, name: &str) -> ApiResult<(String, Secret)> {
    with_account(st, name, |a, s| {
        s.filter(|s| !s.cookie.is_empty())
            .map(|s| (a.id.clone(), Secret::new(s.cookie.as_str())))
    })?
    .ok_or_else(|| ApiError::not_found(format!("{name} has no session")))
}

/// `rbx_err`, and on Unauthorized also mark the session expired, as a launch does.
async fn session_err<H: Host>(h: &H, id: String, ck: Secret, e: RobloxError) -> ApiError {
    if matches!(e, RobloxError::Unauthorized) {
        match blocking(h, move |st| accounts::cmds::expire_session(st, &id, &ck)).await {
            Ok(true) => h.emit_json("accounts://changed", Value::Null),
            Ok(false) => {}
            Err(e) => log::debug!("session state not saved: {}", e.message),
        }
    }
    rbx_err(e)
}

fn rbx_err(e: RobloxError) -> ApiError {
    match e {
        RobloxError::Unauthorized => ApiError::new(
            409,
            "session_expired",
            "Roblox says this account's session expired; sign in again in RMI",
        ),
        e => ApiError::new(502, "roblox_error", e.to_string()),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::platform::{Capabilities, Platform, RobloxInstall};
    use crate::vault::{
        oswrap::FakeOsWrap,
        store::{CreateMode, Vault},
    };
    use std::sync::{Arc, Mutex};

    pub(crate) struct NoPlat;
    impl Platform for NoPlat {
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
        fn roblox_install(&self) -> Option<RobloxInstall> {
            None
        }
        fn open_uri(&self, _: &str) -> std::io::Result<()> {
            panic!("no real launches in tests")
        }
    }

    #[derive(Clone)]
    pub(crate) struct TH(pub Arc<AppState>, pub Arc<Mutex<Vec<String>>>);
    impl Host for TH {
        fn st(&self) -> &AppState {
            &self.0
        }
        fn emit_json(&self, e: &str, _: Value) {
            self.1.lock().unwrap().push(e.into());
        }
    }

    /// Unlocked vault with alice (cookie) and bob (pending). Roblox at a dead port.
    pub(crate) fn host(dir: &std::path::Path) -> TH {
        host_with(dir, Box::new(NoPlat))
    }

    pub(crate) fn host_with(dir: &std::path::Path, plat: Box<dyn Platform>) -> TH {
        host_at(dir, plat, "http://127.0.0.1:9")
    }

    /// `host_with`, Roblox at `roblox`.
    pub(crate) fn host_at(dir: &std::path::Path, plat: Box<dyn Platform>, roblox: &str) -> TH {
        let st = AppState::for_test(dir, plat, roblox, crate::settings::defaults());
        let mut v = Vault::create(&st.paths.vault(), CreateMode::Os, Arc::new(FakeOsWrap)).unwrap();
        let p = v.payload_mut();
        for (id, name, uid) in [("A", "Alice", 1), ("B", "bob", 2)] {
            p.accounts.push(
                serde_json::from_value(json!({
                    "id": id, "userId": uid, "username": name, "displayName": name,
                    "trackerId": format!("{uid}00000"), "session": "ok", "note": "n"
                }))
                .unwrap(),
            );
        }
        p.secrets.insert(
            "A".into(),
            serde_json::from_value(json!({ "cookie": "_|WARNING:-x" })).unwrap(),
        );
        *st.vault.lock().unwrap() = Some(v);
        TH(Arc::new(st), Arc::default())
    }

    fn be(h: &TH) -> AppBackend<TH> {
        let b = AppBackend::default();
        b.set_host(h.clone());
        b
    }

    #[test]
    fn bar_mapping() {
        let r = |v: Value| bar_of(&serde_json::from_value(v).unwrap());
        assert_eq!(r(json!({})), (Bar::default(), false));
        let (b, g) = r(json!({"placeId": 5, "jobId": "j"}));
        assert!(g && b.destination == "5" && b.server == "j" && b.mode == Mode::Specific);
        let (b, _) = r(json!({"placeId": 5}));
        assert!(b.destination == "5" && b.mode == Mode::Any);
        assert_eq!(r(json!({"follow": "@Bob"})).0.destination, "@Bob");
        assert_eq!(
            r(json!({"privateLink": " https://x "})).0.destination,
            "https://x"
        );
    }

    #[tokio::test]
    async fn read_family_maps_accounts_without_secrets() {
        let d = tempfile::tempdir().unwrap();
        let h = host(d.path());
        let b = be(&h);
        let l = b.list_accounts().await.unwrap();
        assert_eq!(l.len(), 2);
        assert_eq!(
            (
                l[0].user_id.unwrap_or(0),
                l[0].session.as_str(),
                l[0].note.as_deref()
            ),
            (1, "ok", Some("n"))
        );
        assert!(!serde_json::to_string(&l).unwrap().contains("WARNING"));
        assert_eq!(b.get_account("ALICE").await.unwrap().username, "Alice");
        assert_eq!(b.get_account("nobody").await.unwrap_err().status, 404);
        assert_eq!(
            b.presence("bob").await.unwrap(),
            json!({ "presence": null })
        );
        assert_eq!(b.cookie("alice").await.unwrap(), "_|WARNING:-x");
        assert_eq!(b.cookie("bob").await.unwrap_err().status, 404);
        // bob has no cookie; alice's Roblox is a dead port.
        assert_eq!(b.block("bob", "x").await.unwrap_err().status, 404);
        assert_eq!(b.blocked("alice").await.unwrap_err().status, 502);
    }

    #[tokio::test]
    async fn edit_family_patches_and_saves() {
        let d = tempfile::tempdir().unwrap();
        let h = host(d.path());
        let b = be(&h);
        let patch: Patch =
            serde_json::from_value(json!({"alias": " Al ", "group": "Main", "extra": {"k": "v"}}))
                .unwrap();
        let a = b.patch_account("alice", patch).await.unwrap();
        assert_eq!(
            (a.alias.as_deref(), a.group.as_str(), a.note.as_deref()),
            (Some("Al"), "Main", Some("n"))
        );
        assert_eq!(a.extra["k"], "v");
        assert!(h
            .1
            .lock()
            .unwrap()
            .contains(&"accounts://changed".to_string()));
        let bad: Patch = serde_json::from_value(json!({"alias": "x".repeat(500)})).unwrap();
        assert_eq!(b.patch_account("alice", bad).await.unwrap_err().status, 400);
        assert_eq!(b.import_cookie("nope").await.unwrap_err().status, 400);
    }

    #[tokio::test]
    async fn launch_family_returns_id_and_status_or_locked() {
        let d = tempfile::tempdir().unwrap();
        let h = host(d.path());
        let b = be(&h);
        let id = b
            .launch(
                "alice",
                LaunchReq {
                    place_id: Some(5),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(b.launch_status(&id).await.unwrap()["launchId"], id.as_str());
        assert_eq!(b.launch_status("nope").await.unwrap_err().status, 404);
        assert_eq!(
            b.launch("nobody", LaunchReq::default())
                .await
                .unwrap_err()
                .status,
            404
        );
        *h.0.vault.lock().unwrap() = None;
        assert_eq!(
            b.launch("alice", LaunchReq::default())
                .await
                .unwrap_err()
                .status,
            423
        );
        assert_eq!(b.list_accounts().await.unwrap_err().status, 423);
    }

    #[tokio::test]
    async fn unset_host_is_503() {
        let b: AppBackend<TH> = AppBackend::default();
        assert_eq!(b.list_accounts().await.unwrap_err().status, 503);
    }

    #[test]
    fn bar_blank_fields_fall_through_and_follow_strips_at() {
        let r = |v: Value| bar_of(&serde_json::from_value(v).unwrap());
        assert_eq!(r(json!({"follow": " @@Bob "})).0.destination, "@Bob");
        let (b, g) = r(json!({"follow": "  ", "privateLink": " ", "placeId": 9}));
        assert!(g && b.destination == "9" && b.mode == Mode::Any);
        let (b, _) = r(json!({"privateLink": "", "jobId": " ", "placeId": 9}));
        assert!(b.server.is_empty() && b.destination == "9");
    }

    #[test]
    fn cmd_err_maps_lock_and_plain_errors() {
        assert_eq!(cmd_err(LOCKED.into()).status, 423);
        assert_eq!(cmd_err(WAS_LOCKED.into()).status, 423);
        let e = cmd_err("nope".into());
        assert_eq!(
            (e.status, e.code, e.message.as_str()),
            (400, "bad_request", "nope")
        );
    }

    #[test]
    fn rbx_err_unauthorized_is_session_expired_else_502() {
        let e = rbx_err(RobloxError::Unauthorized);
        assert_eq!((e.status, e.code), (409, "session_expired"));
        let e = rbx_err(RobloxError::RateLimited);
        assert_eq!((e.status, e.code), (502, "roblox_error"));
    }

    #[tokio::test]
    async fn job_without_place_is_400_before_any_launch() {
        let d = tempfile::tempdir().unwrap();
        let h = host(d.path());
        let b = be(&h);
        let req = LaunchReq {
            job_id: Some("J".into()),
            ..Default::default()
        };
        let e = b.launch("alice", req).await.unwrap_err();
        assert_eq!((e.status, e.code), (400, "bad_request"));
    }

    #[tokio::test]
    async fn friend_paused_launch_is_409_and_stays_paused() {
        let d = tempfile::tempdir().unwrap();
        let h = host(d.path());
        let b = be(&h);
        let flag = |v: bool| {
            let mut g = h.0.vault.lock().unwrap();
            g.as_mut().unwrap().payload_mut().accounts[0].friend_paused = v;
        };
        flag(true);
        let e = b.launch("alice", LaunchReq::default()).await.unwrap_err();
        assert_eq!((e.status, e.code), (409, "friend_paused"));
        let g = h.0.vault.lock().unwrap();
        assert!(
            g.as_ref().unwrap().payload().accounts[0].friend_paused,
            "start_batch not reached"
        );
    }

    #[tokio::test]
    async fn friend_list_running_client_is_never_paused_or_offline() {
        struct Pids;
        impl Platform for Pids {
            fn capabilities(&self) -> Capabilities {
                Capabilities::default()
            }
            fn roblox_install(&self) -> Option<RobloxInstall> {
                None
            }
            fn open_uri(&self, _: &str) -> std::io::Result<()> {
                Ok(())
            }
            fn tracked_pids(&self, t: &str) -> Vec<u32> {
                if t == "100000" {
                    vec![1]
                } else {
                    vec![]
                }
            }
        }
        let d = tempfile::tempdir().unwrap();
        let h = host_with(d.path(), Box::new(Pids));
        let b = be(&h);
        {
            let mut g = h.0.vault.lock().unwrap();
            for a in &mut g.as_mut().unwrap().payload_mut().accounts {
                a.friend_allowed = true;
                a.friend_paused = true;
            }
        }
        let status =
            |b: Vec<FriendAccount>, id: &str| b.into_iter().find(|a| a.id == id).unwrap().status;
        // A has a live client: paused flag alone must not hide it.
        let l = b.friend_list(false).await.unwrap();
        assert_eq!(status(l.clone(), "A"), "online");
        assert_eq!(status(l, "B"), "paused");
        // B unpaused and launching (no pid, no presence yet): Disconnect stays offered.
        h.0.vault
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .payload_mut()
            .accounts[1]
            .friend_paused = false;
        h.0.automation.own_begin(&["B".into()]);
        assert_eq!(status(b.friend_list(false).await.unwrap(), "B"), "starting");
        // Presence without an RMI client is not live: the account runs on another computer.
        let ig = crate::accounts::Presence {
            state: PresenceState::InGame,
            place_id: None,
            universe_id: None,
            game_name: Some("G".into()),
            job_id: None,
            updated_at: String::new(),
        };
        {
            let mut pres = h.0.accounts.presence.lock().unwrap();
            pres.insert("A".into(), ig.clone());
            pres.insert("B".into(), ig);
        }
        h.0.automation
            .own_end(&["B".into()], tokio::time::Instant::now());
        let l = b.friend_list(false).await.unwrap();
        let get = |id: &str| l.iter().find(|a| a.id == id).unwrap();
        assert_eq!(
            (
                get("A").status,
                get("A").game.as_deref(),
                get("A").elsewhere
            ),
            ("inGame", Some("G"), false)
        );
        assert_eq!(
            (
                get("B").status,
                get("B").game.as_deref(),
                get("B").elsewhere
            ),
            ("offline", Some("G"), true)
        );
        // The friend login never gets computer names.
        assert!(l.iter().all(|a| a.computer.is_none()));
        // The owner's does: this computer for a client here; no name for B yet (only Roblox
        // presence says it runs somewhere).
        let here = h.0.remote.device_name();
        let l = b.friend_list(true).await.unwrap();
        let get = |id: &str| l.iter().find(|a| a.id == id).unwrap();
        assert_eq!(get("A").computer.as_deref(), Some(here.as_str()));
        assert_eq!(
            (get("B").elsewhere, get("B").computer.as_deref()),
            (true, None)
        );
        // A paired computer reports B (Roblox user id 2): named, and certain even with no
        // presence and while paused here.
        h.0.remote.note_running("peer", "Mac", vec![2]);
        h.0.accounts.presence.lock().unwrap().clear();
        h.0.vault
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .payload_mut()
            .accounts[1]
            .friend_paused = true;
        let l = b.friend_list(true).await.unwrap();
        let get = |id: &str| l.iter().find(|a| a.id == id).unwrap();
        assert_eq!(
            (
                get("B").status,
                get("B").elsewhere,
                get("B").computer.as_deref()
            ),
            ("paused", true, Some("Mac"))
        );
        // A report for an account with a client here does not move it.
        h.0.remote.note_running("peer", "Mac", vec![1, 2]);
        let l = b.friend_list(true).await.unwrap();
        let a = l.iter().find(|a| a.id == "A").unwrap();
        assert_eq!(
            (a.elsewhere, a.computer.as_deref()),
            (false, Some(here.as_str()))
        );
    }

    #[tokio::test]
    async fn blank_username_is_400_before_any_roblox_call() {
        let d = tempfile::tempdir().unwrap();
        let h = host(d.path());
        let b = be(&h);
        assert_eq!(b.block("alice", " @ ").await.unwrap_err().status, 400);
        assert_eq!(b.unblock("alice", "@").await.unwrap_err().status, 400);
    }

    #[tokio::test]
    async fn empty_cookie_counts_as_no_session() {
        let d = tempfile::tempdir().unwrap();
        let h = host(d.path());
        h.0.vault
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .payload_mut()
            .secrets
            .insert(
                "B".into(),
                serde_json::from_value(json!({ "cookie": "" })).unwrap(),
            );
        let b = be(&h);
        assert_eq!(b.cookie("bob").await.unwrap_err().status, 404);
        assert_eq!(b.block("bob", "x").await.unwrap_err().status, 404);
    }
}

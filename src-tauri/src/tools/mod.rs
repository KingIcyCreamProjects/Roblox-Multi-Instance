//! Account tools IPC (docs/contracts/tools.md, spec §7) plus Discover outfits.
//! Every call uses the account's own cookie; batches run one account at a time and the
//! client's per-account/per-host limiter paces them.

use crate::accounts::cmds::{emit_changed, expire_session, mutate};
use crate::accounts::{find, set_session, Account, SessionState};
use crate::commands::{guard, with_vault, CmdResult, Gen, LOCKED, WAS_LOCKED};
use crate::roblox::api::{account, avatar, economy, friends, thumbnails, users};
use crate::roblox::{RobloxClient, RobloxError, Secret};
use crate::vault::model::VaultPayload;
use crate::AppState;
use serde::Serialize;
use serde_json::{json, Value};
use std::future::Future;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use zeroize::Zeroizing;

pub mod site;

pub const PIN_REQUIRED: &str = "PIN_REQUIRED";
/// Ok row note: the name changed on Roblox but the vault (locked, reopened) did not take it.
pub const NAME_NOT_SAVED: &str = "NAME_NOT_SAVED";
pub const SESSION_EXPIRED: &str = "SESSION_EXPIRED";

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BatchResult {
    pub account_id: String,
    pub ok: bool,
    pub message: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BatchEvent {
    pub batch_id: String,
    pub result: BatchResult,
    pub done: usize,
    pub total: usize,
}

/// Roblox error → contract text: machine code or Roblox's own message in plain words.
pub fn code(e: &RobloxError) -> String {
    static PIN: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)\bpin\b").expect("regex"));
    match e {
        RobloxError::Status {
            code: 400 | 403,
            body_snippet,
        } if PIN.is_match(body_snippet) => PIN_REQUIRED.into(),
        _ => message(e),
    }
}

/// `code` without the PIN rule (PIN unlock itself: "Incorrect PIN", attempt limits).
pub fn message(e: &RobloxError) -> String {
    static MSG: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#""message"\s*:\s*"([^"]+)""#).expect("regex")
    });
    match e {
        RobloxError::Unauthorized => SESSION_EXPIRED.into(),
        RobloxError::Status { body_snippet, .. } => MSG
            .captures(body_snippet)
            .map(|c| c[1].to_owned())
            .unwrap_or_else(|| e.to_string()),
        _ => e.to_string(),
    }
}

pub fn store_cookie(p: &mut VaultPayload, id: &str, c: &Secret) -> CmdResult<()> {
    find(p, id)?;
    p.secrets.entry(id.to_owned()).or_default().cookie = Zeroizing::new(c.expose().to_owned());
    set_session(p, id, SessionState::Ok, None);
    Ok(())
}

/// Replaces the saved password only if one is stored. Returns whether it did.
pub fn update_saved_password(p: &mut VaultPayload, id: &str, next: &str) -> bool {
    match p.secrets.get_mut(id).and_then(|s| s.password.as_mut()) {
        Some(pw) => {
            *pw = Zeroizing::new(next.to_owned());
            true
        }
        None => false,
    }
}

/// Non-secret export (never touches `AccountSecrets`).
pub fn summary(a: &Account) -> Value {
    json!({
        "userId": a.user_id,
        "username": a.username,
        "displayName": a.display_name,
        "alias": a.alias,
        "group": a.group,
        "note": a.note,
        "createdAt": a.created_at,
        "lastLaunchedAt": a.last_launched_at,
        "session": a.session,
        "profileUrl": a.user_id.map(|u| format!("https://www.roblox.com/users/{u}/profile")),
        "exportedAt": crate::accounts::now_iso(),
    })
}

/// Sequential batch: one failure never stops the rest, except a lock: the remaining rows
/// then get the same message without running. `emit` gets every row as it lands.
/// `one` gives `Ok(Some(note))` for a row that worked with a caveat.
pub async fn run_batch<F, Fut>(
    batch_id: &str,
    ids: &[String],
    emit: impl Fn(BatchEvent),
    mut one: F,
) -> Vec<BatchResult>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = CmdResult<Option<String>>>,
{
    let mut out = Vec::with_capacity(ids.len());
    let mut stop: Option<String> = None;
    for id in ids {
        let r = match &stop {
            Some(m) => Err(m.clone()),
            None => one(id.clone()).await,
        };
        if let Err(m) = &r {
            if m == LOCKED || m == WAS_LOCKED {
                stop = Some(m.clone());
            }
        }
        let result = BatchResult {
            account_id: id.clone(),
            ok: r.is_ok(),
            message: r.unwrap_or_else(Some),
        };
        out.push(result.clone());
        emit(BatchEvent {
            batch_id: batch_id.to_owned(),
            result,
            done: out.len(),
            total: ids.len(),
        });
    }
    out
}

/// Cookie + user id of an account, and the lock generation they were read under.
struct Acct {
    id: String,
    user_id: u64,
    ck: Secret,
    gen: Gen,
}

fn acct(st: &AppState, id: &str) -> CmdResult<Acct> {
    let gen = Gen::now(&st.lock_gen);
    with_vault(&st.vault, |v| {
        let p = v.payload();
        let a = &p.accounts[find(p, id)?];
        let ck = p.secrets.get(id).filter(|s| !s.cookie.is_empty());
        match (a.user_id, ck) {
            (Some(user_id), Some(s)) => Ok(Acct {
                id: id.to_owned(),
                user_id,
                ck: Secret::new(s.cookie.as_str()),
                gen,
            }),
            _ => Err(SESSION_EXPIRED.into()),
        }
    })
}

/// The vault is still open and was not locked since `gen`.
fn still_unlocked(st: &AppState, gen: Gen) -> bool {
    let g = guard(&st.vault);
    g.is_some() && gen.current(&st.lock_gen)
}

/// What changed behind the UI's back.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Change {
    /// The vault (`accounts://changed`).
    Accounts,
    /// Roblox's PIN lock state (`state://changed` `["pin"]`): it re-locked, or was unlocked.
    Pin,
}

type Changed<'a> = &'a (dyn Fn(Change) + Sync);

fn ch(app: &AppHandle) -> impl Fn(Change) + Sync + '_ {
    move |c| match c {
        Change::Accounts => emit_changed(app),
        Change::Pin => crate::commands::state_changed(app, &["pin"]),
    }
}

/// Record a dead session on the account (in whatever vault is open now), unless the
/// stored cookie is no longer the one that failed (rotated meanwhile).
fn expire(changed: Changed<'_>, st: &AppState, a: &Acct) {
    match expire_session(st, &a.id, &a.ck) {
        Ok(true) => changed(Change::Accounts),
        Ok(false) => {}
        Err(e) => log::warn!("could not mark session expired: {e}"),
    }
}

/// Store a cookie Roblox rotated. If that fails (vault locked meanwhile) the old cookie
/// is dead, so the account is marked expired.
fn save_rotated(
    changed: Changed<'_>,
    st: &AppState,
    a: &Acct,
    n: Option<&Secret>,
    also: impl FnOnce(&mut VaultPayload),
) -> CmdResult<()> {
    let r = mutate(st, a.gen, |p| {
        if let Some(n) = n {
            store_cookie(p, &a.id, n)?;
        }
        also(p);
        Ok(())
    });
    match r {
        Ok(()) => {
            changed(Change::Accounts);
            Ok(())
        }
        Err(e) => {
            log::warn!("could not save the new session: {e}");
            // Still unlocked: a real save error (disk), not a lock; say so.
            let locked = !a.gen.current(&st.lock_gen);
            if n.is_none() {
                return Err(if locked {
                    "The vault locked before the change was saved here. Unlock it and update the saved details.".into()
                } else {
                    format!("The change was not saved here ({e}). Update the saved details.")
                });
            }
            expire(changed, st, a);
            Err(if locked {
                "Roblox started a new session, but the vault locked before it was saved. Sign in again.".into()
            } else {
                format!("Roblox started a new session, but it was not saved ({e}). Sign in again.")
            })
        }
    }
}

/// Map a Roblox error; a dead session is also recorded on the account, a PIN lock is told
/// to the UI (Roblox re-locked it, so any shown unlock window is over).
fn fail(
    changed: Changed<'_>,
    st: &AppState,
    a: &Acct,
    e: RobloxError,
    map: fn(&RobloxError) -> String,
) -> String {
    let c = map(&e);
    if c == SESSION_EXPIRED {
        expire(changed, st, a);
    } else if c == PIN_REQUIRED {
        changed(Change::Pin);
    } else if !matches!(e, RobloxError::Status { .. }) {
        log::warn!("tool call failed: {e}");
    }
    c
}

/// Look up `id`, run `op` with its cookie, map errors.
async fn with_acct<T, Fut>(
    changed: Changed<'_>,
    st: &AppState,
    id: &str,
    op: impl FnOnce(Arc<RobloxClient>, Arc<Acct>) -> Fut,
) -> CmdResult<(T, Arc<Acct>)>
where
    Fut: Future<Output = crate::roblox::Result<T>>,
{
    with_acct_map(changed, st, id, code, op).await
}

async fn with_acct_map<T, Fut>(
    changed: Changed<'_>,
    st: &AppState,
    id: &str,
    map: fn(&RobloxError) -> String,
    op: impl FnOnce(Arc<RobloxClient>, Arc<Acct>) -> Fut,
) -> CmdResult<(T, Arc<Acct>)>
where
    Fut: Future<Output = crate::roblox::Result<T>>,
{
    let a = Arc::new(acct(st, id)?);
    match op(st.roblox.clone(), a.clone()).await {
        Ok(t) => Ok((t, a)),
        Err(e) => Err(fail(changed, st, &a, e, map)),
    }
}

async fn batch<Fut>(
    app: &AppHandle,
    st: &AppState,
    batch_id: &str,
    ids: &[String],
    op: impl Fn(Arc<RobloxClient>, Arc<Acct>) -> Fut,
) -> Vec<BatchResult>
where
    Fut: Future<Output = crate::roblox::Result<()>>,
{
    let op = &op;
    run_batch(batch_id, ids, emit_batch(app), |id| async move {
        with_acct(&ch(app), st, &id, op).await.map(|_| None)
    })
    .await
}

fn emit_batch(app: &AppHandle) -> impl Fn(BatchEvent) + '_ {
    move |ev| {
        if let Err(e) = app.emit("tools://batch", ev) {
            log::warn!("could not emit tools://batch: {e}");
        }
    }
}

async fn user_id_of(c: &RobloxClient, username: &str) -> CmdResult<u64> {
    let name = username.trim();
    if name.is_empty() {
        return Err("Enter a username.".into());
    }
    users::by_usernames(c, &[name.to_owned()])
        .await
        .map_err(|e| code(&e))?
        .first()
        .map(|u| u.id)
        .ok_or_else(|| format!("No Roblox user is named {name}."))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PinState {
    enabled: bool,
    unlocked_until: Option<String>,
}

/// `unlockedUntil` is seconds left, or (unverified) an epoch time in s or ms.
fn pin_state(p: account::PinStatus, now: i64) -> PinState {
    let until = p.unlocked_until.filter(|s| *s > 0.0).map(|s| {
        let at = if s > 1e12 {
            (s / 1000.0) as i64
        } else if s > 1e9 {
            s as i64
        } else {
            now + s as i64
        };
        chrono::DateTime::from_timestamp(at, 0)
            .unwrap_or_default()
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    });
    PinState {
        enabled: p.is_enabled,
        unlocked_until: until,
    }
}

#[tauri::command]
pub async fn tool_pin_state(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
) -> CmdResult<PinState> {
    let (p, _) = with_acct(
        &ch(&app),
        &state,
        &account_id,
        |c: Arc<RobloxClient>, a: Arc<Acct>| async move { account::pin_status(&c, &a.ck).await },
    )
    .await?;
    Ok(pin_state(p, crate::vault::now_unix()))
}

#[tauri::command]
pub async fn tool_pin_unlock(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    pin: String,
) -> CmdResult<PinState> {
    let pin = Zeroizing::new(pin);
    if pin.len() != 4 || !pin.bytes().all(|b| b.is_ascii_digit()) {
        return Err("The PIN is 4 digits.".into());
    }
    let (p, _) =
        with_acct_map(
            &ch(&app),
            &state,
            &account_id,
            message,
            |c: Arc<RobloxClient>, a: Arc<Acct>| async move {
                account::pin_unlock(&c, &a.ck, &pin).await
            },
        )
        .await?;
    Ok(pin_state(p, crate::vault::now_unix()))
}

#[tauri::command]
pub async fn tool_display_name(
    app: AppHandle,
    state: State<'_, AppState>,
    batch_id: String,
    account_ids: Vec<String>,
    name: String,
) -> CmdResult<Vec<BatchResult>> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Enter a display name.".into());
    }
    let (app, st) = (&app, &*state);
    Ok(
        run_batch(&batch_id, &account_ids, emit_batch(app), |id| async move {
            let changed = ch(app);
            let ((), a) = with_acct(
                &changed,
                st,
                &id,
                |c: Arc<RobloxClient>, a: Arc<Acct>| async move {
                    account::set_display_name(&c, &a.ck, a.user_id, name).await
                },
            )
            .await?;
            Ok(save_display_name(&changed, st, &a, name))
        })
        .await,
    )
}

/// Store `name` on an account that changed it on Roblox, under the lock generation its
/// cookie was read in (a lock + reopen mid-batch then only affects rows read before it).
/// A failed save becomes the row's note.
fn save_display_name(changed: Changed<'_>, st: &AppState, a: &Acct, name: &str) -> Option<String> {
    let saved = mutate(st, a.gen, |p| {
        let i = find(p, &a.id)?;
        p.accounts[i].display_name = name.to_owned();
        Ok(())
    });
    match saved {
        Ok(()) => {
            changed(Change::Accounts);
            None
        }
        Err(e) => {
            log::warn!("display name changed on Roblox, not saved here: {e}");
            Some(NAME_NOT_SAVED.into())
        }
    }
}

#[tauri::command]
pub async fn tool_password(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    current: String,
    next: String,
) -> CmdResult<()> {
    let (current, next) = (Zeroizing::new(current), Zeroizing::new(next));
    if current.is_empty() || next.is_empty() {
        return Err("Enter the current and the new password.".into());
    }
    if *current == *next {
        return Err("The new password is the same as the current one.".into());
    }
    let (cur, nx) = (current.as_str(), next.as_str());
    let (rotated, a) = with_acct(
        &ch(&app),
        &state,
        &account_id,
        |c: Arc<RobloxClient>, a: Arc<Acct>| async move {
            account::change_password(&c, &a.ck, cur, nx).await
        },
    )
    .await?;
    save_rotated(&ch(&app), &state, &a, rotated.as_ref(), |p| {
        update_saved_password(p, &a.id, &next);
    })
    .map_err(|e| match rotated {
        // the "update the saved details" text already covers the no-rotation case
        Some(_) => format!(
            "Password changed on Roblox. {e} The saved password is still the old one; update it after signing in."
        ),
        None => format!("Password changed on Roblox. {e}"),
    })
}

#[derive(Serialize)]
pub struct EmailView {
    masked: Option<String>,
    verified: bool,
}

#[tauri::command]
pub async fn tool_email_get(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
) -> CmdResult<EmailView> {
    let (e, _) = with_acct(
        &ch(&app),
        &state,
        &account_id,
        |c: Arc<RobloxClient>, a: Arc<Acct>| async move { account::email(&c, &a.ck).await },
    )
    .await?;
    Ok(EmailView {
        masked: e.email_address.filter(|s| !s.is_empty()),
        verified: e.verified,
    })
}

#[tauri::command]
pub async fn tool_email_set(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    email: String,
    password: String,
) -> CmdResult<()> {
    let password = Zeroizing::new(password);
    let email = email.trim();
    if !email.contains('@') || password.is_empty() {
        return Err("Enter the new email and the account password.".into());
    }
    with_acct(
        &ch(&app),
        &state,
        &account_id,
        |c: Arc<RobloxClient>, a: Arc<Acct>| async move {
            account::set_email(&c, &a.ck, email, &password).await
        },
    )
    .await
    .map(drop)
}

#[derive(Serialize, Debug, PartialEq)]
pub struct PrivacyOption {
    value: String,
    label: String,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Privacy {
    follow: String,
    private_server_invite: String,
    follow_options: Vec<PrivacyOption>,
    private_server_invite_options: Vec<PrivacyOption>,
}

/// "FriendsAndFollowing" → "Friends and following".
fn label(v: &str) -> String {
    match v {
        "All" | "AllUsers" | "Everyone" => return "Everyone".into(),
        "NoOne" => return "No one".into(),
        _ => {}
    }
    let mut s = String::new();
    for (i, ch) in v.chars().enumerate() {
        if i > 0 && ch.is_uppercase() {
            s.push(' ');
            s.extend(ch.to_lowercase());
        } else {
            s.push(ch);
        }
    }
    s
}

// ponytail: settings-and-options shape is unverified (needs a cookie); keys found by
// substring, values read from the common field names. Pin down after the live test.
const FOLLOW_KEY: &str = "joinme";
const INVITE_KEY: &str = "privateserver";

fn privacy_key<'a>(v: &'a Value, needle: &str) -> Option<&'a str> {
    v.as_object()?
        .keys()
        .find(|k| k.to_lowercase().contains(needle))
        .map(String::as_str)
}

fn setting(v: &Value) -> (String, Vec<PrivacyOption>) {
    let s = |x: &Value| {
        x.as_str()
            .or(x["optionValue"].as_str())
            .or(x["option"]["optionValue"].as_str())
            .or(x["value"].as_str())
            .map(str::to_owned)
    };
    let current = v["currentValue"]
        .as_str()
        .or(v["value"].as_str())
        .or(v.as_str())
        .unwrap_or_default()
        .to_owned();
    let options = v["options"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(s)
        .map(|value| PrivacyOption {
            label: label(&value),
            value,
        })
        .collect();
    (current, options)
}

fn parse_privacy(v: &Value) -> Option<Privacy> {
    let (follow, follow_options) = setting(&v[privacy_key(v, FOLLOW_KEY)?]);
    let (private_server_invite, private_server_invite_options) =
        setting(&v[privacy_key(v, INVITE_KEY)?]);
    Some(Privacy {
        follow,
        private_server_invite,
        follow_options,
        private_server_invite_options,
    })
}

const NO_PRIVACY: &str = "Roblox did not return the privacy settings.";

#[tauri::command]
pub async fn tool_privacy_get(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
) -> CmdResult<Privacy> {
    let (v, _) =
        with_acct(
            &ch(&app),
            &state,
            &account_id,
            |c: Arc<RobloxClient>, a: Arc<Acct>| async move {
                account::privacy_options(&c, &a.ck).await
            },
        )
        .await?;
    parse_privacy(&v).ok_or_else(|| NO_PRIVACY.into())
}

#[tauri::command]
pub async fn tool_privacy_set(
    app: AppHandle,
    state: State<'_, AppState>,
    batch_id: String,
    account_ids: Vec<String>,
    follow: Option<String>,
    private_server_invite: Option<String>,
) -> CmdResult<Vec<BatchResult>> {
    if follow.is_none() && private_server_invite.is_none() {
        return Err("Nothing to change.".into());
    }
    let want = [(FOLLOW_KEY, &follow), (INVITE_KEY, &private_server_invite)];
    Ok(batch(
        &app,
        &state,
        &batch_id,
        &account_ids,
        |c: Arc<RobloxClient>, a: Arc<Acct>| async move {
            let v = account::privacy_options(&c, &a.ck).await?;
            let mut patch = serde_json::Map::new();
            for (needle, val) in &want {
                if let Some(val) = val {
                    let k = privacy_key(&v, needle)
                        .ok_or_else(|| RobloxError::Decode(NO_PRIVACY.into()))?;
                    patch.insert(k.to_owned(), json!(val));
                }
            }
            account::set_privacy(&c, &a.ck, &Value::Object(patch)).await
        },
    )
    .await)
}

const SIGNED_OUT_HERE: &str = "Signed out everywhere, including here. Sign in again.";

#[tauri::command]
pub async fn tool_signout_others(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
) -> CmdResult<()> {
    signout_others(&ch(&app), &state, &account_id).await
}

async fn signout_others(changed: Changed<'_>, st: &AppState, id: &str) -> CmdResult<()> {
    let (n, a) = with_acct(changed, st, id, |c: Arc<RobloxClient>, a: Arc<Acct>| async move {
        account::signout_others(&c, &a.ck).await
    })
    .await?;
    match n {
        Some(n) => save_rotated(changed, st, &a, Some(&n), |_| {}),
        None => {
            // every session ended, ours too
            log::warn!("sign out others: Roblox issued no new cookie");
            expire(changed, st, &a);
            Err(SIGNED_OUT_HERE.into())
        }
    }
}

#[tauri::command]
pub async fn tool_friend_request(
    app: AppHandle,
    state: State<'_, AppState>,
    batch_id: String,
    account_ids: Vec<String>,
    username: String,
) -> CmdResult<Vec<BatchResult>> {
    let target = user_id_of(&state.roblox, &username).await?;
    Ok(batch(
        &app,
        &state,
        &batch_id,
        &account_ids,
        |c: Arc<RobloxClient>, a: Arc<Acct>| async move {
            let r = friends::request(&c, &a.ck, target).await?;
            let msg = if r.is_captcha_required {
                "Roblox wants a captcha for this request. Send it on the website."
            } else if !r.success {
                "Roblox did not send the request."
            } else {
                return Ok(());
            };
            Err(RobloxError::Message(msg.into()))
        },
    )
    .await)
}

#[tauri::command]
pub async fn tool_block(
    app: AppHandle,
    state: State<'_, AppState>,
    batch_id: String,
    account_ids: Vec<String>,
    username: String,
    block: bool,
) -> CmdResult<Vec<BatchResult>> {
    let target = user_id_of(&state.roblox, &username).await?;
    Ok(batch(
        &app,
        &state,
        &batch_id,
        &account_ids,
        |c: Arc<RobloxClient>, a: Arc<Acct>| async move {
            account::block(&c, &a.ck, target, block).await
        },
    )
    .await)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockedUser {
    user_id: u64,
    username: String,
    display_name: String,
}

#[tauri::command]
pub async fn tool_blocked_list(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
) -> CmdResult<Vec<BlockedUser>> {
    let (us, _) = with_acct(
        &ch(&app),
        &state,
        &account_id,
        |c: Arc<RobloxClient>, a: Arc<Acct>| async move {
            let (ids, more) = account::blocked_ids(&c, &a.ck).await?;
            if more {
                log::warn!("blocked list cut at {} pages", account::BLOCK_PAGES);
            }
            users::by_ids(&c, &ids).await
        },
    )
    .await?;
    Ok(us
        .into_iter()
        .map(|u| BlockedUser {
            user_id: u.id,
            username: u.name,
            display_name: u.display_name,
        })
        .collect())
}

#[tauri::command]
pub async fn tool_unblock_all(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
) -> CmdResult<Value> {
    let st: &AppState = &state;
    let ((count, failed, more), _) = with_acct(
        &ch(&app),
        st,
        &account_id,
        |c: Arc<RobloxClient>, a: Arc<Acct>| async move {
            let (ids, more) = account::blocked_ids(&c, &a.ck).await?;
            let (mut ok, mut bad) = (0, 0);
            for id in &ids {
                // Can run for many minutes: a lock stops it (no more calls with the cookie).
                if !still_unlocked(st, a.gen) {
                    return Err(RobloxError::Message(LOCKED.into()));
                }
                match account::block(&c, &a.ck, *id, false).await {
                    Ok(()) => ok += 1,
                    // the rest would fail the same way
                    Err(e) if matches!(code(&e).as_str(), SESSION_EXPIRED | PIN_REQUIRED) => {
                        return Err(e)
                    }
                    Err(e) => {
                        log::warn!("unblock failed: {}", code(&e));
                        bad += 1;
                    }
                }
            }
            Ok((ok, bad, more))
        },
    )
    .await?;
    Ok(json!({"count": count, "failed": failed, "truncated": more}))
}

#[tauri::command]
pub async fn tool_robux(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
) -> CmdResult<Value> {
    let (r, _) =
        with_acct(
            &ch(&app),
            &state,
            &account_id,
            |c: Arc<RobloxClient>, a: Arc<Acct>| async move {
                economy::robux(&c, &a.ck, a.user_id).await
            },
        )
        .await?;
    Ok(json!({"robux": r}))
}

#[tauri::command]
pub async fn tool_info(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
) -> CmdResult<Value> {
    let (u, _) = with_acct(
        &ch(&app),
        &state,
        &account_id,
        |c: Arc<RobloxClient>, a: Arc<Acct>| async move { users::user(&c, a.user_id).await },
    )
    .await?;
    let age = chrono::DateTime::parse_from_rfc3339(&u.created)
        .map(|t| (chrono::Utc::now() - t.to_utc()).num_days())
        .unwrap_or(0);
    Ok(json!({
        "userId": u.id,
        "username": u.name,
        "displayName": u.display_name,
        "created": u.created,
        "ageDays": age,
        "profileUrl": format!("https://www.roblox.com/users/{}/profile", u.id),
    }))
}

#[tauri::command]
pub async fn tool_export_summary(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
) -> CmdResult<Option<Value>> {
    use tauri_plugin_dialog::DialogExt;
    let (json, name) = with_vault(&state.vault, |v| {
        let p = v.payload();
        let a = &p.accounts[find(p, &account_id)?];
        Ok((summary(a), a.username.clone()))
    })?;
    let d = app
        .dialog()
        .file()
        .add_filter("JSON", &["json"])
        .set_file_name(format!("{name}-summary.json"));
    let Some(path) = tauri::async_runtime::spawn_blocking(move || d.blocking_save_file())
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    let path = path
        .into_path()
        .map_err(|_| "Pick a file on this PC.".to_string())?;
    let text = serde_json::to_string_pretty(&json).expect("json serializes");
    std::fs::write(&path, text).map_err(|e| format!("Could not save the file: {e}"))?;
    Ok(Some(json!({"path": path.display().to_string()})))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outfit {
    id: u64,
    name: String,
    thumbnail: Option<String>,
}

#[tauri::command]
pub async fn outfits_list(state: State<'_, AppState>, username: String) -> CmdResult<Value> {
    let c = &state.roblox;
    let user_id = user_id_of(c, &username).await?;
    let os = avatar::outfits(c, user_id).await.map_err(|e| code(&e))?;
    let ids: Vec<u64> = os.iter().map(|o| o.id).collect();
    // thumbnails are cosmetic: a failure leaves them empty
    let thumbs = thumbnails::outfits(c, &ids).await.unwrap_or_default();
    let outfits: Vec<Outfit> = os
        .into_iter()
        .map(|o| Outfit {
            thumbnail: thumbs
                .iter()
                .find(|t| t.target_id == o.id && t.state == "Completed")
                .and_then(|t| t.image_url.clone()),
            id: o.id,
            name: o.name,
        })
        .collect();
    Ok(json!({"userId": user_id, "outfits": outfits}))
}

#[tauri::command]
pub async fn outfit_wear(
    app: AppHandle,
    state: State<'_, AppState>,
    outfit_id: u64,
    account_id: String,
) -> CmdResult<Value> {
    let (missing, _) = with_acct(&ch(&app), &state, &account_id, |c: Arc<RobloxClient>, a: Arc<Acct>| async move {
        avatar::wear(&c, &a.ck, outfit_id).await
    })
    .await?;
    let missing: Vec<Value> = missing
        .into_iter()
        .map(|(id, name)| {
            json!({"assetId": id, "name": name, "url": format!("https://www.roblox.com/catalog/{id}")})
        })
        .collect();
    Ok(json!({"missing": missing}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::{upsert, Identity};
    use std::sync::atomic::Ordering::SeqCst;
    use std::sync::Mutex;
    /// Mock Roblox (401 on `authenticated`, CSRF + rotation on sign-out) and a mock
    /// Tauri app holding an unlocked vault with one account using `cookie`.
    async fn env(cookie: &str) -> (tempfile::TempDir, AppState, String) {
        use axum::{
            http::HeaderMap,
            response::IntoResponse,
            routing::{get, post},
        };
        let r = axum::Router::new()
            .route(
                "/v1/users/authenticated",
                get(|| async { axum::http::StatusCode::UNAUTHORIZED }),
            )
            .route(
                "/v1/logoutfromallsessionsandreauthenticate",
                post(|h: HeaderMap| async move {
                    if h.get("x-csrf-token").is_none() {
                        return (axum::http::StatusCode::FORBIDDEN, [("x-csrf-token", "t")])
                            .into_response();
                    }
                    let rotate = h
                        .get("cookie")
                        .and_then(|v| v.to_str().ok())
                        .is_some_and(|c| c.contains("rotate"));
                    let mut res = "{}".into_response();
                    if rotate {
                        res.headers_mut().append(
                            "set-cookie",
                            ".ROBLOSECURITY=; expires=Thu, 01 Jan 1970 00:00:00 GMT"
                                .parse()
                                .unwrap(),
                        );
                        res.headers_mut().append(
                            "set-cookie",
                            ".ROBLOSECURITY=FRESH; HttpOnly".parse().unwrap(),
                        );
                    }
                    res
                }),
            );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, r).await.unwrap() });
        let d = tempfile::tempdir().unwrap();
        let st = crate::test_state(
            d.path(),
            RobloxClient::new(crate::roblox::BaseUrls::all(&url)),
        );
        let who = Identity {
            user_id: 7,
            username: "bob".into(),
            display_name: "Bob".into(),
        };
        let id = mutate(&st, Gen::now(&st.lock_gen), |p| {
            Ok(upsert(p, &who, Some(Zeroizing::new(cookie.into())), None).1)
        })
        .unwrap();
        (d, st, id)
    }

    fn session_and_cookie(st: &AppState, id: &str) -> (SessionState, String) {
        with_vault(&st.vault, |v| {
            let p = v.payload();
            Ok((
                p.accounts[find(p, id)?].session,
                p.secrets[id].cookie.to_string(),
            ))
        })
        .unwrap()
    }

    #[tokio::test]
    async fn command_401_marks_session_expired() {
        let (_d, st, id) = env("dead").await;
        let n = std::sync::atomic::AtomicUsize::new(0);
        let changed = |c| {
            assert_eq!(c, Change::Accounts);
            n.fetch_add(1, SeqCst);
        };
        let e =
            with_acct(
                &changed,
                &st,
                &id,
                |c: Arc<RobloxClient>, a: Arc<Acct>| async move {
                    users::authenticated(&c, &a.ck).await
                },
            )
            .await
            .err()
            .unwrap();
        assert_eq!(e, SESSION_EXPIRED);
        assert_eq!(session_and_cookie(&st, &id).0, SessionState::Expired);
        assert_eq!(n.load(SeqCst), 1, "accounts://changed emitted");
    }

    #[tokio::test]
    async fn pin_required_tells_the_ui() {
        let (_d, st, id) = env("c").await;
        let a = acct(&st, &id).unwrap();
        let seen = Mutex::new(vec![]);
        let e = RobloxError::Status {
            code: 403,
            body_snippet: r#"{"message":"Account PIN is locked"}"#.into(),
        };
        let c = fail(&|c| seen.lock().unwrap().push(c), &st, &a, e, code);
        assert_eq!(c, PIN_REQUIRED);
        assert_eq!(*seen.lock().unwrap(), [Change::Pin]);
    }

    #[tokio::test]
    async fn signout_stores_rotated_cookie() {
        let (_d, st, id) = env("rotate-me").await;
        signout_others(&|_| {}, &st, &id).await.unwrap();
        assert_eq!(
            session_and_cookie(&st, &id),
            (SessionState::Ok, "FRESH".to_string())
        );
    }

    #[tokio::test]
    async fn signout_without_new_cookie_expires() {
        let (_d, st, id) = env("plain").await;
        let e = signout_others(&|_| {}, &st, &id).await.unwrap_err();
        assert_eq!(e, SIGNED_OUT_HERE);
        assert_eq!(session_and_cookie(&st, &id).0, SessionState::Expired);
    }

    #[tokio::test]
    async fn stale_401_keeps_rotated_session() {
        let (_d, st, id) = env("old").await;
        let a = acct(&st, &id).unwrap();
        mutate(&st, a.gen, |p| store_cookie(p, &id, &Secret::new("new"))).unwrap();
        fail(&|_| {}, &st, &a, RobloxError::Unauthorized, code);
        assert_eq!(
            session_and_cookie(&st, &id),
            (SessionState::Ok, "new".to_string())
        );
    }

    #[tokio::test]
    async fn site_window_lock_check() {
        let (_d, st, _id) = env("c").await;
        let gen = Gen::now(&st.lock_gen);
        assert!(still_unlocked(&st, gen));
        st.lock_gen.fetch_add(1, SeqCst);
        assert!(!still_unlocked(&st, gen), "locked + reopened since");
        let gen = Gen::now(&st.lock_gen);
        *guard(&st.vault) = None;
        assert!(!still_unlocked(&st, gen), "locked now");
    }

    #[tokio::test]
    async fn display_name_saved_or_reported() {
        let (_d, st, id) = env("c").await;
        let a = acct(&st, &id).unwrap();
        assert_eq!(save_display_name(&|_| {}, &st, &a, "Neo"), None);
        let name = |st: &AppState| {
            with_vault(&st.vault, |v| {
                Ok(v.payload().accounts[0].display_name.clone())
            })
            .unwrap()
        };
        assert_eq!(name(&st), "Neo");
        st.lock_gen.fetch_add(1, SeqCst); // locked + reopened since `a` was read
        assert_eq!(
            save_display_name(&|_| {}, &st, &a, "Late").as_deref(),
            Some(NAME_NOT_SAVED)
        );
        assert_eq!(name(&st), "Neo");
        // a row read after the reopen saves under its own generation
        let b = acct(&st, &id).unwrap();
        assert_eq!(save_display_name(&|_| {}, &st, &b, "Fresh"), None);
        assert_eq!(name(&st), "Fresh");
    }

    #[tokio::test]
    async fn rotated_cookie_lost_on_lock_expires() {
        let (_d, st, id) = env("rotate-me").await;
        let a = acct(&st, &id).unwrap();
        st.lock_gen.fetch_add(1, SeqCst); // a lock + re-open happened mid-call
        let e = save_rotated(&|_| {}, &st, &a, Some(&Secret::new("X")), |_| {}).unwrap_err();
        assert!(e.contains("Sign in again"), "{e}");
        let e = save_rotated(&|_| {}, &st, &a, None, |_| {}).unwrap_err();
        assert!(e.starts_with("The vault locked before the change"), "{e}");
        assert_eq!(
            session_and_cookie(&st, &id),
            (SessionState::Expired, "rotate-me".to_string())
        );
    }

    fn payload() -> (VaultPayload, String) {
        let mut p = VaultPayload::default();
        let who = Identity {
            user_id: 7,
            username: "bob".into(),
            display_name: "Bob".into(),
        };
        let (_, id) = upsert(
            &mut p,
            &who,
            Some(Zeroizing::new("OLDCOOKIE".into())),
            Some(Zeroizing::new("oldpw-secret".into())),
        );
        (p, id)
    }

    #[test]
    fn error_codes() {
        let st = |code, b: &str| RobloxError::Status {
            code,
            body_snippet: b.into(),
        };
        assert_eq!(code(&RobloxError::Unauthorized), SESSION_EXPIRED);
        assert_eq!(
            code(&st(
                403,
                r#"{"errors":[{"message":"Account PIN is locked."}]}"#
            )),
            PIN_REQUIRED
        );
        assert_eq!(
            code(&st(
                400,
                r#"{"errors":[{"code":1,"message":"You can change your display name once every 7 days."}]}"#
            )),
            "You can change your display name once every 7 days."
        );
        assert_ne!(code(&st(403, r#"{"message":"spinning"}"#)), PIN_REQUIRED);
        // PIN unlock shows Roblox's words
        assert_eq!(
            message(&st(403, r#"{"errors":[{"message":"Incorrect PIN"}]}"#)),
            "Incorrect PIN"
        );
        assert_eq!(message(&RobloxError::Unauthorized), SESSION_EXPIRED);
        assert_eq!(
            code(&RobloxError::Message("No captcha".into())),
            "No captcha"
        );
        assert!(code(&st(500, "")).contains("HTTP 500"));
    }

    #[test]
    fn expired_cookie_and_password_updates() {
        let (mut p, id) = payload();
        set_session(&mut p, &id, SessionState::Expired, None);
        assert_eq!(p.accounts[0].session, SessionState::Expired);
        store_cookie(&mut p, &id, &Secret::new("NEWCOOKIE")).unwrap();
        assert_eq!(p.secrets[&id].cookie.as_str(), "NEWCOOKIE");
        assert_eq!(p.accounts[0].session, SessionState::Ok);
        assert!(update_saved_password(&mut p, &id, "newpw"));
        assert_eq!(
            p.secrets[&id].password.as_deref().unwrap().as_str(),
            "newpw"
        );
        p.secrets.get_mut(&id).unwrap().password = None;
        assert!(
            !update_saved_password(&mut p, &id, "x"),
            "none stored stays none"
        );
        assert!(p.secrets[&id].password.is_none());
    }

    #[test]
    fn export_has_no_secrets() {
        let (p, _) = payload();
        let s = summary(&p.accounts[0]).to_string();
        assert!(s.contains("\"username\":\"bob\""), "{s}");
        assert!(
            !s.contains("OLDCOOKIE") && !s.contains("oldpw-secret"),
            "{s}"
        );
        assert!(s.contains("https://www.roblox.com/users/7/profile"));
    }

    #[tokio::test]
    async fn batch_continues_after_failure_and_emits_each() {
        let ids: Vec<String> = ["a", "b", "c"].map(String::from).to_vec();
        let seen = Mutex::new(vec![]);
        let r = run_batch(
            "B1",
            &ids,
            |e| {
                seen.lock()
                    .unwrap()
                    .push((e.batch_id, e.result.account_id, e.done, e.total))
            },
            |id| async move {
                if id == "a" {
                    Err(PIN_REQUIRED.to_string())
                } else {
                    Ok(None)
                }
            },
        )
        .await;
        assert_eq!(
            r.iter().map(|x| x.ok).collect::<Vec<_>>(),
            [false, true, true]
        );
        assert_eq!(r[0].message.as_deref(), Some(PIN_REQUIRED));
        let s = seen.into_inner().unwrap();
        assert_eq!(s.len(), 3);
        assert_eq!(s[2], ("B1".into(), "c".into(), 3, 3));
    }

    #[tokio::test]
    async fn batch_stops_on_lock() {
        let ids: Vec<String> = ["a", "b", "c"].map(String::from).to_vec();
        let calls = Mutex::new(vec![]);
        let n = std::sync::atomic::AtomicUsize::new(0);
        let r = run_batch(
            "B",
            &ids,
            |_| {
                n.fetch_add(1, SeqCst);
            },
            |id| {
                calls.lock().unwrap().push(id.clone());
                async move {
                    if id == "b" {
                        Err(LOCKED.to_string())
                    } else {
                        Ok(None)
                    }
                }
            },
        )
        .await;
        assert_eq!(*calls.lock().unwrap(), ["a", "b"], "c never runs");
        assert_eq!(r[2].message.as_deref(), Some(LOCKED));
        assert_eq!(n.load(SeqCst), 3, "every row still emitted");
    }

    #[test]
    fn privacy_parses_leniently() {
        let v = json!({
            "whoCanJoinMeInExperiences": {"currentValue": "Friends", "options": [
                {"option": {"optionValue": "All"}}, {"option": {"optionValue": "FriendsAndFollowing"}}, "NoOne"
            ]},
            "whoCanInviteMeToPrivateServers": {"currentValue": "NoOne", "options": [{"optionValue": "Friends"}]}
        });
        let p = parse_privacy(&v).unwrap();
        assert_eq!(
            (p.follow.as_str(), p.private_server_invite.as_str()),
            ("Friends", "NoOne")
        );
        assert_eq!(
            p.follow_options
                .iter()
                .map(|o| o.label.as_str())
                .collect::<Vec<_>>(),
            ["Everyone", "Friends and following", "No one"]
        );
        assert_eq!(p.private_server_invite_options[0].value, "Friends");
        assert!(parse_privacy(&json!({})).is_none());
    }

    #[test]
    fn pin_until() {
        let p = |u| account::PinStatus {
            is_enabled: true,
            unlocked_until: u,
        };
        assert_eq!(pin_state(p(None), 0).unlocked_until, None);
        assert_eq!(
            pin_state(p(Some(300.0)), 1_700_000_000)
                .unlocked_until
                .as_deref(),
            Some("2023-11-14T22:18:20Z")
        );
        assert_eq!(
            pin_state(p(Some(1_700_000_300.0)), 0)
                .unlocked_until
                .as_deref(),
            Some("2023-11-14T22:18:20Z")
        );
    }

    #[test]
    fn pin_until_ms_zero_and_negative() {
        let p = |u| account::PinStatus {
            is_enabled: true,
            unlocked_until: u,
        };
        assert_eq!(pin_state(p(Some(0.0)), 1).unlocked_until, None);
        assert_eq!(pin_state(p(Some(-5.0)), 1).unlocked_until, None);
        assert_eq!(
            pin_state(p(Some(1_700_000_300_000.0)), 0)
                .unlocked_until
                .as_deref(),
            Some("2023-11-14T22:18:20Z")
        );
    }

    #[test]
    fn pin_rule_needs_400_or_403_and_a_whole_word() {
        let st = |code, b: &str| RobloxError::Status {
            code,
            body_snippet: b.into(),
        };
        assert_eq!(
            code(&st(400, r#"{"message":"PIN required"}"#)),
            PIN_REQUIRED
        );
        assert_ne!(
            code(&st(404, r#"{"message":"PIN required"}"#)),
            PIN_REQUIRED
        );
        assert_ne!(
            code(&st(403, r#"{"message":"Pinterest is down"}"#)),
            PIN_REQUIRED
        );
    }

    #[test]
    fn privacy_setting_shapes() {
        assert_eq!(
            setting(&json!("Friends")),
            ("Friends".to_string(), Vec::<PrivacyOption>::new())
        );
        let (cur, opts) = setting(&json!({
            "value": "NoOne",
            "options": ["All", {"value": "Friends"}, 3]
        }));
        assert_eq!(cur, "NoOne");
        assert_eq!(
            opts.iter().map(|o| o.value.as_str()).collect::<Vec<_>>(),
            ["All", "Friends"]
        );
    }

    #[tokio::test]
    async fn username_lookup_errors() {
        use axum::{http::StatusCode, response::IntoResponse, routing::post, Json};
        let app = axum::Router::new().route(
            "/v1/usernames/users",
            post(|Json(b): Json<Value>| async move {
                match b["usernames"][0].as_str() {
                    Some("Bob") => Json(json!({"data": [{"requestedUsername": "Bob",
                    "hasVerifiedBadge": false, "id": 7, "name": "bob", "displayName": "Bob"}]}))
                    .into_response(),
                    Some("Boom") => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
                    _ => Json(json!({"data": []})).into_response(),
                }
            }),
        );
        let c = crate::roblox::mock(app).await;
        assert_eq!(user_id_of(&c, "  ").await.unwrap_err(), "Enter a username.");
        assert_eq!(user_id_of(&c, " Bob ").await.unwrap(), 7);
        assert_eq!(
            user_id_of(&c, " Nobody ").await.unwrap_err(),
            "No Roblox user is named Nobody."
        );
        assert!(user_id_of(&c, "Boom")
            .await
            .unwrap_err()
            .contains("HTTP 500"));
    }

    #[tokio::test]
    async fn account_without_cookie_or_unknown_is_session_expired() {
        let (_d, st, id) = env("").await;
        assert_eq!(
            acct(&st, "nope").err().as_deref(),
            Some("Account not found.")
        );
        assert_eq!(acct(&st, &id).err().as_deref(), Some(SESSION_EXPIRED));
    }

    #[tokio::test]
    async fn batch_stops_on_was_locked_too() {
        let ids: Vec<String> = ["a", "b", "c"].map(String::from).to_vec();
        let calls = Mutex::new(vec![]);
        let r = run_batch(
            "B",
            &ids,
            |_| {},
            |id| {
                calls.lock().unwrap().push(id.clone());
                async move {
                    if id == "a" {
                        Err(WAS_LOCKED.to_string())
                    } else {
                        Ok(None)
                    }
                }
            },
        )
        .await;
        assert_eq!(*calls.lock().unwrap(), ["a"], "b and c never run");
        assert_eq!(r[1].message.as_deref(), Some(WAS_LOCKED));
    }

    #[tokio::test]
    async fn new_cookie_unsaved_while_unlocked_asks_sign_in() {
        let (_d, st, _id) = env("rotate-me").await;
        let a = Acct {
            id: "missing".into(),
            user_id: 7,
            ck: Secret::new("rotate-me"),
            gen: Gen::now(&st.lock_gen),
        };
        let e = save_rotated(&|_| {}, &st, &a, Some(&Secret::new("X")), |_| {}).unwrap_err();
        assert!(
            e.starts_with(
                "Roblox started a new session, but it was not saved (Account not found.)"
            ),
            "{e}"
        );
        assert!(e.ends_with("Sign in again."), "{e}");
    }
}

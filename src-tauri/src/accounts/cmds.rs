//! Account IPC (docs/contracts/accounts.md). Secrets leave Rust only via `account_reveal`
//! and the clipboard in `account_copy`, both behind a 2-minute re-auth.

use super::*;
use crate::commands::{
    guard, keys, require_password, verr, with_vault, CmdResult, Gen, LOCKED, WAS_LOCKED,
};
use crate::roblox::{api::users, Secret};
use crate::vault::format::Mode;
use crate::AppState;
use std::sync::atomic::Ordering::SeqCst;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

pub const REAUTH_REQUIRED: &str = "REAUTH_REQUIRED";
const REAUTH_SECS: i64 = 120;
// ponytail: the Undo toast shows 8 s but pauses on hover; Rust holds far longer so the toast is
// the only clock that matters. Exact sync would need a dismiss IPC.
const UNDO_SECS: u64 = 120;
const CLIPBOARD_SECS: u64 = 30;

pub(crate) fn emit_changed(app: &AppHandle) {
    if let Err(e) = app.emit("accounts://changed", ()) {
        log::warn!("could not emit accounts://changed: {e}");
    }
}

/// Run `f` on the payload and save; on any error the payload is rolled back. `start` is the
/// lock generation seen before slow work, so nothing lands in a vault re-opened since.
pub(crate) fn mutate<T>(
    st: &AppState,
    start: Gen,
    f: impl FnOnce(&mut VaultPayload) -> CmdResult<T>,
) -> CmdResult<T> {
    let mut g = guard(&st.vault);
    start.check(&st.lock_gen)?; // before LOCKED: a lock since `start` reports WAS_LOCKED
    let v = g.as_mut().ok_or(LOCKED)?;
    let before = v.payload().clone();
    let r = f(v.payload_mut()).and_then(|t| v.save().map(|_| t).map_err(verr));
    if r.is_err() {
        *v.payload_mut() = before;
    }
    r
}

/// Roblox rejected `cookie` (401): mark `id` expired in whatever vault is open now, unless
/// that cookie was replaced meanwhile. `Ok(true)` = changed; the caller emits `accounts://changed`.
pub(crate) fn expire_session(st: &AppState, id: &str, cookie: &Secret) -> CmdResult<bool> {
    mutate(st, gen(st), |p| {
        Ok(set_session(
            p,
            id,
            SessionState::Expired,
            Some(cookie.expose()),
        ))
    })
}

fn gen(st: &AppState) -> Gen {
    Gen::now(&st.lock_gen)
}

pub(crate) fn thumb_file(st: &AppState, user_id: u64, size: u32) -> std::path::PathBuf {
    st.paths
        .cache
        .join("thumbs")
        .join(format!("{user_id}-{size}.png"))
}

#[derive(Serialize)]
pub struct AccountsList {
    accounts: Vec<Value>,
    groups: Vec<String>,
    #[serde(rename = "groupTargets")]
    group_targets: BTreeMap<String, LaunchTarget>,
}

#[tauri::command]
pub async fn accounts_list(state: State<'_, AppState>) -> CmdResult<AccountsList> {
    let presence = state
        .accounts
        .presence
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    with_vault(&state.vault, |v| {
        let p = v.payload();
        let accounts = p
            .accounts
            .iter()
            .map(|a| {
                let head = a
                    .user_id
                    .map(|u| thumb_file(&state, u, 48))
                    .filter(|f| f.is_file())
                    .map(|f| f.display().to_string());
                view(a, p.secrets.get(&a.id), presence.get(&a.id), head)
            })
            .collect();
        Ok(AccountsList {
            accounts,
            groups: groups_view(p),
            group_targets: group_targets_view(p),
        })
    })
}

/// A line that passed validation, applied with the rest of its batch in one save.
type Valid = (
    usize,
    Identity,
    Option<Zeroizing<String>>,
    Option<Zeroizing<String>>,
);

fn apply_batch(
    app: &AppHandle,
    st: &AppState,
    start: Gen,
    mut out: Vec<AddResult>,
    valid: Vec<Valid>,
) -> CmdResult<Vec<AddResult>> {
    if !valid.is_empty() {
        let added = mutate(st, start, |p| {
            Ok(valid
                .into_iter()
                .map(|(line, who, c, pw)| AddResult {
                    line,
                    status: upsert(p, &who, c, pw).0,
                    username: Some(who.username),
                    message: None,
                })
                .collect::<Vec<_>>())
        })?;
        out.extend(added);
        emit_changed(app);
        super::sync::kick_headshots(app);
    }
    out.sort_by_key(|r| r.line);
    Ok(out)
}

#[tauri::command]
pub async fn accounts_add_cookies(
    app: AppHandle,
    state: State<'_, AppState>,
    text: String,
) -> CmdResult<Vec<AddResult>> {
    let text = Zeroizing::new(text);
    let start = gen(&state);
    let (mut out, mut valid) = (Vec::new(), Vec::new());
    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        if raw.trim().is_empty() {
            continue;
        }
        let Some(c) = normalize_cookie(raw).map(Zeroizing::new) else {
            out.push(AddResult::invalid(
                line,
                "This line is not a Roblox session cookie.",
            ));
            continue;
        };
        start.check(&state.lock_gen)?;
        match users::authenticated(&state.roblox, &Secret::new(c.as_str())).await {
            Ok(u) => valid.push((
                line,
                Identity {
                    user_id: u.id,
                    username: u.name,
                    display_name: u.display_name,
                },
                Some(c),
                None,
            )),
            Err(crate::roblox::RobloxError::Unauthorized) => out.push(AddResult::invalid(
                line,
                "Roblox says this cookie is signed out or expired.",
            )),
            Err(e) => out.push(AddResult::invalid(line, e.to_string())),
        }
    }
    apply_batch(&app, &state, start, out, valid)
}

#[tauri::command]
pub async fn accounts_add_pending(
    app: AppHandle,
    state: State<'_, AppState>,
    text: String,
) -> CmdResult<Vec<AddResult>> {
    let text = Zeroizing::new(text);
    let start = gen(&state);
    let (mut out, mut ok) = (Vec::new(), Vec::new());
    for (i, raw) in text.lines().enumerate() {
        if raw.trim().is_empty() {
            continue;
        }
        match parse_user_pass(raw) {
            Some(up) => ok.push((i + 1, up)),
            None => out.push(AddResult::invalid(
                i + 1,
                "Use username:password, one per line.",
            )),
        }
    }
    let names: Vec<String> = ok.iter().map(|(_, (u, _))| u.clone()).collect();
    let found = if names.is_empty() {
        Vec::new()
    } else {
        users::by_usernames(&state.roblox, &names)
            .await
            .map_err(|e| e.to_string())?
    };
    let mut valid = Vec::new();
    for (line, (name, pw)) in ok {
        match found
            .iter()
            .find(|u| u.requested_username.eq_ignore_ascii_case(&name))
        {
            Some(u) => valid.push((
                line,
                Identity {
                    user_id: u.id,
                    username: u.name.clone(),
                    display_name: u.display_name.clone(),
                },
                None,
                Some(pw),
            )),
            None => out.push(AddResult::invalid(
                line,
                format!("No Roblox user is named {name}."),
            )),
        }
    }
    apply_batch(&app, &state, start, out, valid)
}

#[tauri::command]
pub async fn accounts_update(
    app: AppHandle,
    state: State<'_, AppState>,
    ids: Vec<String>,
    patch: Value,
) -> CmdResult<()> {
    // all or nothing: one save, one event
    mutate(&state, gen(&state), |p| {
        ids.iter().try_for_each(|id| update(p, id, &patch))
    })?;
    emit_changed(&app);
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoToken {
    undo_token: String,
}

#[tauri::command]
pub async fn accounts_remove(
    app: AppHandle,
    state: State<'_, AppState>,
    ids: Vec<String>,
) -> CmdResult<UndoToken> {
    let g = gen(&state);
    let gone = mutate(&state, g, |p| Ok(remove(p, &ids)))?;
    let token = new_id();
    hold(&app, g, token.clone(), gone);
    emit_changed(&app);
    Ok(UndoToken { undo_token: token })
}

/// Keep removed items undoable for `UNDO_SECS`, unless a lock (which clears the map) came
/// after `g`: checked under the map's mutex, so a lock can't be undone by a late insert.
fn hold(app: &AppHandle, g: Gen, token: String, items: Vec<Removed>) {
    let st = app.state::<AppState>();
    let mut m = st
        .accounts
        .removed
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !g.current(&st.lock_gen) {
        return; // dropping zeroizes the secrets
    }
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    m.insert(token.clone(), (seq, items));
    drop(m);
    let h = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(UNDO_SECS)).await;
        expire_hold(
            &mut h
                .state::<AppState>()
                .accounts
                .removed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            &token,
            seq,
        );
    });
}

/// Timer end of hold `seq`: a later re-hold of the same token keeps its own timer.
fn expire_hold(m: &mut HashMap<String, (u64, Vec<Removed>)>, token: &str, seq: u64) {
    if m.get(token).is_some_and(|(s, _)| *s == seq) {
        m.remove(token);
    }
}

#[tauri::command]
pub async fn accounts_undo_remove(
    app: AppHandle,
    state: State<'_, AppState>,
    undo_token: String,
) -> CmdResult<()> {
    let g = gen(&state); // before taking the items: a lock in between must drop them
    let items = state
        .accounts
        .removed
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&undo_token)
        .ok_or("Undo window has passed.")?
        .1;
    let r = mutate(&state, g, |p| {
        restore(p, items.clone());
        Ok(())
    });
    if r.is_err() {
        hold(&app, g, undo_token, items); // undoable again, with a fresh timer
    }
    r?;
    emit_changed(&app);
    Ok(())
}

#[tauri::command]
pub async fn accounts_reorder(
    app: AppHandle,
    state: State<'_, AppState>,
    group: String,
    ids: Vec<String>,
) -> CmdResult<()> {
    mutate(&state, gen(&state), |p| reorder(p, &group, &ids))?;
    emit_changed(&app);
    Ok(())
}

#[tauri::command]
pub async fn groups_reorder(
    app: AppHandle,
    state: State<'_, AppState>,
    groups: Vec<String>,
) -> CmdResult<()> {
    mutate(&state, gen(&state), |p| set_groups(p, &groups))?;
    emit_changed(&app);
    Ok(())
}

#[tauri::command]
pub async fn group_rename(
    app: AppHandle,
    state: State<'_, AppState>,
    from: String,
    to: String,
) -> CmdResult<()> {
    mutate(&state, gen(&state), |p| rename_group(p, &from, &to))?;
    emit_changed(&app);
    Ok(())
}

/// `target: null` clears the group's saved target.
#[tauri::command]
pub async fn group_target_set(
    app: AppHandle,
    state: State<'_, AppState>,
    group: String,
    target: Option<LaunchTarget>,
) -> CmdResult<()> {
    mutate(&state, gen(&state), |p| set_group_target(p, &group, target))?;
    emit_changed(&app);
    Ok(())
}

fn reauth_ok(until: i64, now: i64) -> bool {
    now < until
}

/// Clipboard/reveal text for `field`. Secret fields need `reauthed`.
fn field_text(
    p: &VaultPayload,
    id: &str,
    field: &str,
    reauthed: bool,
) -> CmdResult<Zeroizing<String>> {
    let secret = matches!(field, "cookie" | "password");
    if secret && !reauthed {
        return Err(REAUTH_REQUIRED.into());
    }
    let a = &p.accounts[find(p, id)?];
    let s = p.secrets.get(id);
    let t = match field {
        "username" => a.username.clone(),
        "userId" => a
            .user_id
            .ok_or("This account has no user id yet.")?
            .to_string(),
        "profileLink" => format!(
            "https://www.roblox.com/users/{}/profile",
            a.user_id.ok_or("This account has no user id yet.")?
        ),
        "cookie" => match s.filter(|s| !s.cookie.is_empty()) {
            Some(s) => return Ok(s.cookie.clone()),
            None => return Err("No cookie saved for this account.".into()),
        },
        "password" => match s.and_then(|s| s.password.clone()) {
            Some(p) => return Ok(p),
            None => return Err("No password saved for this account.".into()),
        },
        _ => return Err("Unknown field.".into()),
    };
    Ok(Zeroizing::new(t))
}

/// Run `f` on the shared clipboard handle (kept alive: on Linux the owner serves the data).
fn clip<T>(
    rt: &Runtime,
    f: impl FnOnce(&mut arboard::Clipboard) -> Result<T, arboard::Error>,
) -> CmdResult<T> {
    let mut g = rt
        .clipboard
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if g.is_none() {
        *g = Some(
            arboard::Clipboard::new().map_err(|e| format!("Could not use the clipboard: {e}"))?,
        );
    }
    f(g.as_mut().expect("set above")).map_err(|e| format!("Could not use the clipboard: {e}"))
}

fn reauthed(st: &AppState) -> bool {
    reauth_ok(
        st.accounts.reauth_until.load(SeqCst),
        crate::vault::now_unix(),
    )
}

#[tauri::command]
pub async fn account_copy(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    field: String,
) -> CmdResult<()> {
    let g = gen(&state);
    let ok = reauthed(&state);
    let text = with_vault(&state.vault, |v| field_text(v.payload(), &id, &field, ok))?;
    let secret = matches!(field.as_str(), "cookie" | "password");
    copy_text(app, &state, g, &text, secret)
}

/// Copy the automation API token, or `template` with `{token}` filled in (the curl / ws
/// examples): a secret like a cookie (30 s clear, cleared on lock, kept out of Windows
/// clipboard history), not the webview clipboard.
#[tauri::command]
pub async fn automation_token_copy(
    app: AppHandle,
    state: State<'_, AppState>,
    template: Option<String>,
) -> CmdResult<()> {
    let g = gen(&state);
    let tok = Zeroizing::new(state.automation.token());
    let text = token_text(&tok, template.as_deref())?;
    copy_text(app, &state, g, &text, true)
}

/// Copy the friend password: a secret like the token (30 s clear, cleared on lock, kept out
/// of Windows clipboard history).
#[tauri::command]
pub async fn friend_password_copy(app: AppHandle, state: State<'_, AppState>) -> CmdResult<()> {
    let g = gen(&state);
    crate::commands::friend_token(&app).await;
    let pw = Zeroizing::new(state.friend.password()?);
    copy_text(app, &state, g, &pw, true)
}

fn token_text(tok: &str, template: Option<&str>) -> CmdResult<Zeroizing<String>> {
    if tok.is_empty() {
        return Err("No token yet. Turn on Automation first.".into());
    }
    match template {
        None => Ok(Zeroizing::new(tok.to_owned())),
        Some(t) if t.contains("{token}") => Ok(Zeroizing::new(t.replace("{token}", tok))),
        Some(_) => Err("The template needs {token}.".into()),
    }
}

/// Put `text` on the clipboard; `secret`: hidden from history, cleared after 30 s and on a
/// lock (`g` = lock generation before `text` was read).
fn copy_text(
    app: AppHandle,
    state: &AppState,
    g: Gen,
    text: &Zeroizing<String>,
    secret: bool,
) -> CmdResult<()> {
    // Write first: a failed write must not orphan the previous secret's timer/lock clear.
    clip(&state.accounts, |c| {
        if !secret {
            return c.set_text(text.as_str());
        }
        let set = c.set();
        #[cfg(windows)]
        let set = {
            use arboard::SetExtWindows;
            set.exclude_from_history().exclude_from_cloud()
        };
        #[cfg(target_os = "macos")]
        let set = arboard::SetExtApple::exclude_from_history(set);
        #[cfg(target_os = "linux")]
        let set = arboard::SetExtLinux::exclude_from_history(set);
        set.text(text.as_str())
    })?;
    let seq = state.accounts.clip_seq.fetch_add(1, SeqCst) + 1;
    *state
        .accounts
        .clip_secret
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = secret.then(|| text.clone());
    if secret && !g.current(&state.lock_gen) {
        clear_secret_clip(&state.accounts); // locked mid-copy: on_lock already ran
        return Err(WAS_LOCKED.into());
    }
    if secret {
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(CLIPBOARD_SECS)).await;
            let st = app.state::<AppState>();
            if st.accounts.clip_seq.load(SeqCst) == seq {
                clear_secret_clip(&st.accounts); // else a newer copy owns the clipboard
            }
        });
    }
    Ok(())
}

/// Clear the clipboard if it still holds the last copied secret (30 s timer, vault lock).
pub(crate) fn clear_secret_clip(rt: &Runtime) {
    let Some(text) = rt
        .clip_secret
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
    else {
        return;
    };
    let same =
        clip(rt, |c| c.get_text()).is_ok_and(|t| Zeroizing::new(t).as_str() == text.as_str());
    if same {
        if let Err(e) = clip(rt, |c| c.clear()) {
            log::warn!("{e}");
        }
    }
}

#[tauri::command]
pub async fn account_reveal(
    state: State<'_, AppState>,
    id: String,
    field: String,
) -> CmdResult<String> {
    if !matches!(field.as_str(), "cookie" | "password") {
        return Err("Unknown field.".into());
    }
    let ok = reauthed(&state);
    with_vault(&state.vault, |v| {
        field_text(v.payload(), &id, &field, ok).map(|t| t.to_string())
    })
}

#[tauri::command]
pub async fn reauth(
    app: AppHandle,
    state: State<'_, AppState>,
    password: Option<String>,
) -> CmdResult<()> {
    let password = password.map(Zeroizing::new);
    let start = gen(&state);
    if keys(&state.vault)?.mode() == Mode::Os {
        os_confirm(&app).await?;
    } else {
        require_password(&state.vault, password.as_deref().map(|s| s.as_str()))?;
    }
    state
        .accounts
        .reauth_until
        .store(crate::vault::now_unix() + REAUTH_SECS, SeqCst);
    // a lock in between (which resets the window) must win
    if !start.current(&state.lock_gen) {
        state.accounts.reauth_until.store(0, SeqCst);
        return Err(WAS_LOCKED.into());
    }
    Ok(())
}

/// OS-mode user presence for a bulk secret export (password mode re-types the password):
/// never weaker than copying one cookie. A fresh `reauth` window counts.
pub(crate) async fn os_presence(app: &AppHandle, st: &AppState) -> CmdResult<()> {
    if keys(&st.vault)?.mode() == Mode::Os && !reauthed(st) {
        os_confirm(app).await?;
    }
    Ok(())
}

const CONFIRM_MSG: &str = "Roblox Multi Instance wants to show or copy a saved secret.";

/// Windows Hello user-presence check on the main window.
#[cfg(windows)]
async fn os_confirm(app: &AppHandle) -> CmdResult<()> {
    use windows::core::HSTRING;
    use windows::Security::Credentials::UI::{
        UserConsentVerificationResult as R, UserConsentVerifier,
    };
    use windows::Win32::System::WinRT::IUserConsentVerifierInterop;
    let hwnd = app
        .get_webview_window("main")
        .ok_or("The main window is gone.")?
        .hwnd()
        .map_err(|e| e.to_string())?;
    let hwnd = hwnd.0 as isize;
    let r = tauri::async_runtime::spawn_blocking(move || -> windows::core::Result<R> {
        let f = windows::core::factory::<UserConsentVerifier, IUserConsentVerifierInterop>()?;
        // SAFETY: a live top-level window handle of this process.
        let op: windows_future::IAsyncOperation<R> = unsafe {
            f.RequestVerificationForWindowAsync(
                windows::Win32::Foundation::HWND(hwnd as _),
                &HSTRING::from(CONFIRM_MSG),
            )?
        };
        op.join()
    })
    .await
    .map_err(|e| e.to_string())?;
    match r {
        Ok(R::Verified) => Ok(()),
        Ok(R::Canceled) => Err("Cancelled.".into()),
        Ok(R::DeviceNotPresent | R::NotConfiguredForUser | R::DisabledByPolicy) => Err(
            "Windows Hello is not set up, so Windows can't confirm it's you. Set up a PIN in Windows Settings, or give the vault a password."
                .into(),
        ),
        Ok(_) => Err("Windows could not confirm it's you.".into()),
        Err(e) => {
            log::warn!("Windows Hello prompt failed: {e}");
            Err("Windows could not confirm it's you.".into())
        }
    }
}

// ponytail: plain confirm dialog on mac/linux (presence, not identity); LocalAuthentication/polkit later.
#[cfg(not(windows))]
async fn os_confirm(app: &AppHandle) -> CmdResult<()> {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
    let d = app
        .dialog()
        .message(CONFIRM_MSG)
        .title("Confirm it's you")
        .buttons(MessageDialogButtons::OkCancel);
    let ok = tauri::async_runtime::spawn_blocking(move || d.blocking_show())
        .await
        .map_err(|e| e.to_string())?;
    ok.then_some(()).ok_or_else(|| "Cancelled.".into())
}

#[tauri::command]
pub async fn signin_start(
    app: AppHandle,
    account_id: Option<String>,
    username_hint: Option<String>,
    password: Option<String>,
) -> CmdResult<()> {
    let password = password.map(Zeroizing::new).filter(|p| !p.is_empty());
    super::signin::start(app, account_id, username_hint, password).await
}

#[tauri::command]
pub async fn signin_kill_stray(app: AppHandle) -> CmdResult<usize> {
    Ok(super::signin::kill_stray(&app))
}

#[tauri::command]
/// `false` = a check is already running (nothing started).
pub async fn accounts_validate(
    app: AppHandle,
    state: State<'_, AppState>,
    ids: Option<Vec<String>>,
) -> CmdResult<bool> {
    if guard(&state.vault).is_none() {
        return Err(LOCKED.into());
    }
    let Some(f) = super::sync::Flight::claim(&app) else {
        return Ok(false);
    };
    tauri::async_runtime::spawn(async move { super::sync::validate(&app, ids, f).await });
    Ok(true)
}

#[tauri::command]
pub async fn avatar_full(
    app: AppHandle,
    state: State<'_, AppState>,
    user_id: u64,
) -> CmdResult<Option<String>> {
    if guard(&state.vault).is_none() {
        return Err(LOCKED.into());
    }
    Ok(super::sync::avatar_full(&app, user_id).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_hold_timer_spares_a_re_hold() {
        let mut m = HashMap::new();
        m.insert("t".to_string(), (2, Vec::new()));
        expire_hold(&mut m, "t", 1); // first hold's timer
        assert!(m.contains_key("t"));
        expire_hold(&mut m, "t", 2);
        assert!(m.is_empty());
    }

    #[test]
    fn token_copy_text() {
        assert_eq!(token_text("abc", None).unwrap().as_str(), "abc");
        assert_eq!(
            token_text("abc", Some("Bearer {token} x"))
                .unwrap()
                .as_str(),
            "Bearer abc x"
        );
        assert!(token_text("abc", Some("no placeholder")).is_err());
        assert!(token_text("", None).is_err());
    }

    #[test]
    fn copy_secret_needs_reauth() {
        let mut p = VaultPayload::default();
        let who = Identity {
            user_id: 3,
            username: "u".into(),
            display_name: "U".into(),
        };
        let (_, id) = upsert(&mut p, &who, Some(Zeroizing::new("ck".into())), None);
        for f in ["cookie", "password"] {
            assert_eq!(field_text(&p, &id, f, false).unwrap_err(), REAUTH_REQUIRED);
        }
        assert_eq!(field_text(&p, &id, "cookie", true).unwrap().as_str(), "ck");
        assert!(field_text(&p, &id, "password", true).is_err(), "none saved");
        assert_eq!(
            field_text(&p, &id, "username", false).unwrap().as_str(),
            "u"
        );
        assert_eq!(
            field_text(&p, &id, "profileLink", false).unwrap().as_str(),
            "https://www.roblox.com/users/3/profile"
        );
        assert!(field_text(&p, "nope", "username", true).is_err());
        assert!(reauth_ok(100 + REAUTH_SECS, 100 + REAUTH_SECS - 1));
        assert!(!reauth_ok(100 + REAUTH_SECS, 100 + REAUTH_SECS));
        assert!(!reauth_ok(0, 1), "never re-authed");
    }

    #[test]
    fn field_text_missing_pieces() {
        let mut p = VaultPayload::default();
        let who = Identity {
            user_id: 4,
            username: "u".into(),
            display_name: "U".into(),
        };
        let (_, id) = upsert(&mut p, &who, None, None);
        // legacy row: no user id, no secrets yet
        p.accounts
            .push(serde_json::from_value(json!({"id": "nouid", "username": "n"})).unwrap());
        assert_eq!(field_text(&p, &id, "userId", false).unwrap().as_str(), "4");
        assert_eq!(
            field_text(&p, "nouid", "userId", false).unwrap_err(),
            "This account has no user id yet."
        );
        assert_eq!(
            field_text(&p, "nouid", "profileLink", false).unwrap_err(),
            "This account has no user id yet."
        );
        assert_eq!(
            field_text(&p, "nouid", "cookie", true).unwrap_err(),
            "No cookie saved for this account."
        );
        assert_eq!(
            field_text(&p, &id, "password", true).unwrap_err(),
            "No password saved for this account."
        );
        assert_eq!(
            field_text(&p, "nope", "cookie", true).unwrap_err(),
            "Account not found."
        );
        assert_eq!(
            field_text(&p, &id, "bogus", true).unwrap_err(),
            "Unknown field."
        );
    }

    #[test]
    fn expire_session_ignores_replaced_cookie() {
        let d = tempfile::tempdir().unwrap();
        let st = crate::test_state(
            d.path(),
            crate::roblox::RobloxClient::new(crate::roblox::BaseUrls::all("http://127.0.0.1:9")),
        );
        let who = Identity {
            user_id: 3,
            username: "u".into(),
            display_name: "U".into(),
        };
        let id = mutate(&st, Gen::now(&st.lock_gen), |p| {
            Ok(upsert(p, &who, Some(Zeroizing::new("ck".into())), None).1)
        })
        .unwrap();
        assert!(!expire_session(&st, &id, &Secret::new("old")).unwrap());
        assert!(expire_session(&st, &id, &Secret::new("ck")).unwrap());
        assert!(
            !expire_session(&st, &id, &Secret::new("ck")).unwrap(),
            "already expired"
        );
        let s = guard(&st.vault).as_ref().unwrap().payload().accounts[0].session;
        assert_eq!(s, SessionState::Expired);
    }

    #[test]
    fn mutate_rolls_back_and_respects_lock_generation() {
        let d = tempfile::tempdir().unwrap();
        let st = crate::test_state(
            d.path(),
            crate::roblox::RobloxClient::new(crate::roblox::BaseUrls::all("http://127.0.0.1:9")),
        );
        let who = Identity {
            user_id: 3,
            username: "u".into(),
            display_name: "U".into(),
        };
        let r: CmdResult<()> = mutate(&st, Gen::now(&st.lock_gen), |p| {
            upsert(p, &who, None, None);
            Err("nope".into())
        });
        assert_eq!(r, Err("nope".to_string()));
        assert!(guard(&st.vault)
            .as_ref()
            .unwrap()
            .payload()
            .accounts
            .is_empty());
        let g = Gen::now(&st.lock_gen);
        st.lock_gen
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let r = mutate(&st, g, |p| {
            upsert(p, &who, None, None);
            Ok(())
        });
        assert_eq!(r, Err(WAS_LOCKED.to_string()));
        assert!(guard(&st.vault)
            .as_ref()
            .unwrap()
            .payload()
            .accounts
            .is_empty());
    }
}

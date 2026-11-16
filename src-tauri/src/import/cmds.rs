//! Import IPC (spec §11.4). Old files are only read. The decrypted set stays in Rust
//! (`Runtime::import`, 5 min, zeroized on drop); the UI only gets non-secret rows.

use super::{
    decrypt, detect, parse_games_json, parse_ini, read_capped, Encoding, ImportedAccount,
    LegacyGame, LegacyPrefs,
};
use crate::accounts::{
    self,
    cmds::{emit_changed, thumb_file},
    set_session, AddStatus, Identity, Rect, SessionState, DEFAULT_GROUP,
};
use crate::commands::{guard, persist, verr, with_vault, CmdResult, Gen, LOCKED};
use crate::discover::favorites::{self, Favorite};
use crate::launch::recents::{self, Recent};
use crate::vault::{model::VaultPayload, store::Vault};
use crate::AppState;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::PoisonError;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};
use zeroize::Zeroizing;

const TTL: Duration = Duration::from_secs(300);
/// Error code (UI: `hasCode(e, "PREVIEW_EXPIRED")`, text in src/lib/ipc.ts CODE_TEXT).
const EXPIRED: &str = "PREVIEW_EXPIRED";

pub struct Preview {
    id: String,
    at: Instant,
    /// Lock generation at preview time; a lock since makes it stale.
    gen: Gen,
    accounts: Vec<ImportedAccount>,
    prefs: Option<LegacyPrefs>,
    recents: Vec<LegacyGame>,
    favorites: Vec<LegacyGame>,
}

fn folder(path: &str) -> CmdResult<PathBuf> {
    let p = PathBuf::from(path.trim());
    if p.is_absolute() && p.is_dir() {
        Ok(p)
    } else {
        Err("Choose a folder.".into())
    }
}

#[derive(Serialize)]
pub struct Picked {
    path: String,
}

#[tauri::command]
pub async fn import_pick_folder(app: AppHandle) -> CmdResult<Option<Picked>> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |p| {
        let _ = tx.send(p);
    });
    Ok(rx
        .await
        .ok()
        .flatten()
        .and_then(|p| p.into_path().ok())
        .map(|p| Picked {
            path: p.display().to_string(),
        }))
}

#[derive(Serialize)]
pub struct AccountFile {
    encoding: Option<&'static str>,
    error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectOut {
    account_data: Option<AccountFile>,
    settings: bool,
    recents: bool,
    favorites: bool,
}

#[tauri::command]
pub async fn import_detect(path: String) -> CmdResult<DetectOut> {
    let d = detect(&folder(&path)?);
    Ok(DetectOut {
        account_data: d.account_data.map(|(_, r)| match r {
            Ok(e) => AccountFile {
                encoding: Some(match e {
                    Encoding::Locked => "locked",
                    Encoding::Dpapi => "dpapi",
                    Encoding::Plain => "plain",
                }),
                error: None,
            },
            Err(e) => AccountFile {
                encoding: None,
                error: Some(e.to_string()),
            },
        }),
        settings: d.settings.is_some(),
        recents: d.recents.is_some(),
        favorites: d.favorites.is_some(),
    })
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    key: String,
    user_id: Option<u64>,
    username: String,
    alias: Option<String>,
    group: String,
    will_update: bool,
    has_cookie: bool,
    has_password: bool,
    headshot: Option<String>,
}

#[derive(Serialize)]
pub struct Games {
    recents: usize,
    favorites: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewOut {
    preview_id: String,
    rows: Vec<Row>,
    prefs: Option<LegacyPrefs>,
    games: Games,
    /// Per-file problems (unreadable settings or game lists).
    notes: Vec<String>,
}

/// Non-secret preview rows; `key` = index into the preview.
pub(crate) fn rows(
    p: &VaultPayload,
    list: &[ImportedAccount],
    head: impl Fn(u64) -> Option<String>,
) -> Vec<Row> {
    list.iter()
        .enumerate()
        .map(|(i, a)| Row {
            key: i.to_string(),
            user_id: a.user_id,
            username: a.username.clone(),
            alias: a.alias.clone(),
            group: a.group.clone(),
            will_update: a
                .user_id
                .is_some_and(|u| p.accounts.iter().any(|x| x.user_id == Some(u))),
            has_cookie: a.cookie.is_some(),
            has_password: a.password.is_some(),
            headshot: a.user_id.and_then(&head),
        })
        .collect()
}

#[tauri::command]
pub async fn import_preview(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    password: Option<String>,
    accounts: bool,
) -> CmdResult<PreviewOut> {
    let password = password.map(Zeroizing::new);
    let start = Gen::now(&state.lock_gen);
    // locked vault: refuse before the slow Argon2 run
    with_vault(&state.vault, |_| Ok(()))?;
    let d = detect(&folder(&path)?);
    let (accounts, skipped) = match d.account_data {
        // accounts not wanted: never decrypt, no cookies held
        _ if !accounts => (Vec::new(), 0),
        Some((p, Ok(_))) => {
            let bytes = Zeroizing::new(read_capped(&p).map_err(|e| e.to_string())?);
            // Argon2 (256 MiB) off the async runtime; the old password drops with the closure
            tauri::async_runtime::spawn_blocking(move || {
                decrypt(&bytes, password.as_ref().map(|s| s.as_str()))
            })
            .await
            .map_err(|_| "Could not read the account file.")?
            .map_err(|e| e.to_string())?
        }
        // unreadable/unrecognised was already shown by import_detect
        _ => (Vec::new(), 0),
    };
    let mut notes = Vec::new();
    if skipped > 0 {
        notes.push(format!(
            "{skipped} account entries without a username were skipped."
        ));
    }
    let mut text = |p: Option<PathBuf>| {
        let p = p?;
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned());
        match read_capped(&p) {
            Ok(b) => Some(String::from_utf8_lossy(&b).into_owned()),
            Err(e) => {
                notes.push(format!("{}: {e}", name.unwrap_or_default()));
                None
            }
        }
    };
    let prefs = text(d.settings).map(|t| parse_ini(&t));
    let recents_t = text(d.recents);
    let favorites_t = text(d.favorites);
    let mut games = |t: Option<String>, name: &str| {
        let t = t?;
        let g = parse_games_json(&t);
        let arr = serde_json::from_str::<Value>(t.trim_start_matches('\u{feff}'))
            .is_ok_and(|v| v.is_array());
        if g.is_empty() && !arr {
            notes.push(format!("{name}: {}", super::ImportError::Unrecognised));
        }
        Some(g)
    };
    let recents = games(recents_t, "RecentGames.json").unwrap_or_default();
    let favorites = games(favorites_t, "FavoriteGames.json").unwrap_or_default();
    let id = accounts::new_id();
    // slot written under the vault guard (lock order: vault, then import), and only if no
    // lock happened since we started; on_lock clears it afterwards otherwise
    let out = with_vault(&state.vault, |v| {
        start.check(&state.lock_gen)?;
        let out = PreviewOut {
            preview_id: id.clone(),
            rows: rows(v.payload(), &accounts, |u| {
                Some(thumb_file(&state, u, 48))
                    .filter(|f| f.is_file())
                    .map(|f| f.display().to_string())
            }),
            prefs: prefs.clone(),
            games: Games {
                recents: recents.len(),
                favorites: favorites.len(),
            },
            notes,
        };
        *state
            .accounts
            .import
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Preview {
            id: id.clone(),
            at: Instant::now(),
            gen: start,
            accounts,
            prefs,
            recents,
            favorites,
        });
        Ok(out)
    })?;
    // drop (zeroize) at expiry even if never committed
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(TTL).await;
        let st = app.state::<AppState>();
        let mut g = st
            .accounts
            .import
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if g.as_ref().is_some_and(|p| p.id == id) {
            *g = None;
        }
    });
    Ok(out)
}

/// The preview `id` if still fresh and from this unlock (`gen`); an expired or stale one is
/// dropped (zeroized) here.
pub(crate) fn fresh<'a>(
    slot: &'a mut Option<Preview>,
    id: &str,
    now: Instant,
    gen: Gen,
) -> Option<&'a Preview> {
    if slot
        .as_ref()
        .is_some_and(|p| now.duration_since(p.at) >= TTL || p.gen != gen)
    {
        *slot = None;
    }
    slot.as_ref().filter(|p| p.id == id)
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Added,
    Updated,
    Skipped,
    Failed,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CommitRow {
    key: String,
    /// RMI account id (added/updated), so the UI can follow its session check.
    id: Option<String>,
    username: String,
    status: Status,
    message: Option<String>,
}

fn fill(dst: &mut Option<String>, src: &Option<String>) {
    if dst.as_deref().is_none_or(str::is_empty) {
        dst.clone_from(src);
    }
}

pub(crate) const KEPT: &str = "Kept the current sign-in.";

/// Upsert one legacy account. New: every mapped field. Existing (same userId): RMI metadata
/// kept, only empty fields filled; a signed-in (session ok) account keeps its cookie and
/// password (password only filled when empty), otherwise the old cookie replaces it (§6.2).
fn merge(
    p: &mut VaultPayload,
    uid: u64,
    i: &ImportedAccount,
) -> (AddStatus, String, Option<&'static str>) {
    let old = p.accounts.iter().find(|a| a.user_id == Some(uid));
    let who = match old {
        Some(a) => Identity {
            user_id: uid,
            username: a.username.clone(),
            display_name: a.display_name.clone(),
        },
        None => Identity {
            user_id: uid,
            username: i.username.clone(),
            display_name: i.username.clone(),
        },
    };
    let keep = old.filter(|a| a.session == SessionState::Ok);
    let never_launched = old.is_none_or(|a| a.last_launched_at.is_none());
    // existing account: the old password only fills an empty one
    let pw_empty = old.is_none_or(|a| {
        p.secrets
            .get(&a.id)
            .and_then(|s| s.password.as_ref())
            .is_none_or(|s| s.is_empty())
    });
    let password = i.password.clone().filter(|_| pw_empty);
    let cookie = if keep.is_some() {
        None
    } else {
        i.cookie.clone()
    };
    let msg = keep.map(|_| KEPT);
    let group = accounts::clean_group(&i.group).unwrap_or_else(|_| DEFAULT_GROUP.into());
    let order = accounts::next_order(p, &group);
    let replaced = cookie.is_some();
    let (st, id) = accounts::upsert(p, &who, cookie, password);
    // A tracker id another account (vault or this batch) holds would close its clients too.
    let tracker = i.tracker_id.as_ref().filter(|t| {
        (1..=20).contains(&t.len())
            && t.bytes().all(|b| b.is_ascii_digit())
            && !p.accounts.iter().any(|o| o.id != id && o.tracker_id == **t)
    });
    if replaced {
        // not checked yet: the validation pass after commit decides ok/expired
        set_session(p, &id, SessionState::Unknown, None);
    }
    let a = p
        .accounts
        .iter_mut()
        .find(|a| a.id == id)
        .expect("upserted");
    if replaced {
        a.last_validated_at = None;
    }
    if st == AddStatus::Added {
        a.group = group;
        a.order = order;
    }
    // keep the old id so Roblox sees continuity, unless RMI already launched with its own
    if never_launched {
        if let Some(t) = tracker {
            a.tracker_id.clone_from(t);
        }
    }
    fill(&mut a.alias, &i.alias);
    fill(&mut a.note, &i.note);
    if a.launch_target.is_none() {
        a.launch_target = i.launch_target.as_ref().map(|l| accounts::LaunchTarget {
            place_id: l.place_id,
            job_id: l.job_id.clone(),
            private_link: None,
            mode: None,
        });
    }
    a.fps_cap = a.fps_cap.or(i.fps_cap);
    if a.window_rect.is_none() {
        a.window_rect = i.window_rect.as_ref().and_then(|r| {
            Some(Rect {
                x: r.x.try_into().ok()?,
                y: r.y.try_into().ok()?,
                w: r.w.try_into().ok()?,
                h: r.h.try_into().ok()?,
            })
        });
    }
    if a.last_launched_at.is_none() {
        a.last_launched_at.clone_from(&i.last_launched_at);
    }
    for (k, v) in &i.extra {
        a.extra.entry(k.clone()).or_insert_with(|| v.clone());
    }
    (st, id, msg)
}

pub(crate) fn apply(p: &mut VaultPayload, picks: &[(String, &ImportedAccount)]) -> Vec<CommitRow> {
    let mut seen = HashSet::new();
    picks
        .iter()
        .map(|(key, i)| {
            let row = |id, status, message: Option<&str>| CommitRow {
                key: key.clone(),
                id,
                username: i.username.clone(),
                status,
                message: message.map(Into::into),
            };
            let Some(uid) = i.user_id else {
                return row(
                    None,
                    Status::Failed,
                    Some("The old file has no Roblox user id for this account."),
                );
            };
            if !seen.insert(uid) {
                return row(None, Status::Skipped, Some("Listed twice in the old file."));
            }
            let (st, id, msg) = merge(p, uid, i);
            let st = if st == AddStatus::Added {
                Status::Added
            } else {
                Status::Updated
            };
            row(Some(id), st, msg)
        })
        .collect()
}

/// All picks in one vault save, with a required backup of the current file first.
pub(crate) fn commit_vault(
    v: &mut Vault,
    picks: &[(String, &ImportedAccount)],
) -> CmdResult<Vec<CommitRow>> {
    let mut p = v.payload().clone();
    let out = apply(&mut p, picks);
    if out.iter().any(|r| r.id.is_some()) {
        v.replace_payload(p).map_err(verr)?;
    } else {
        p.wipe();
    }
    Ok(out)
}

/// Old game name, or the placeId when empty; at most 100 chars.
fn game_name(g: &LegacyGame) -> String {
    let n = g.name.trim();
    if n.is_empty() {
        g.place_id.to_string()
    } else {
        n.chars().take(100).collect()
    }
}

/// Thumbnail URL only when it is https on `*.rbxcdn.com` (the UI loads it).
fn cdn_only(url: &Option<String>) -> Option<String> {
    let u = url::Url::parse(url.as_deref()?).ok()?;
    let sub = u.host_str()?.strip_suffix(".rbxcdn.com")?;
    let ok = u.scheme() == "https"
        && u.username().is_empty()
        && u.password().is_none()
        && !sub.is_empty()
        && !sub.ends_with('.');
    ok.then(|| u.into())
}

/// Legacy prefs → settings patch (clamped on save).
pub(crate) fn prefs_patch(p: &LegacyPrefs) -> Value {
    let mut v = json!({});
    if let Some(x) = p.launch_delay_ms {
        v["launch"]["delayMs"] = json!(x);
    }
    if let Some(x) = p.fps_unlock {
        v["fps"]["unlock"] = json!(x);
    }
    if let Some(x) = p.fps_value {
        v["fps"]["value"] = json!(x);
    }
    if let Some(x) = p.multi_client {
        v["multi"]["enabled"] = json!(x);
    }
    if let Some(x) = p.recents_max {
        v["recents"]["max"] = json!(x);
    }
    v
}

#[derive(Serialize)]
pub struct CommitOut {
    results: Vec<CommitRow>,
    notes: Vec<String>,
    /// Settings or games failed; the preview is kept so the commit can run again.
    retry: bool,
}

#[tauri::command]
pub async fn import_commit(
    app: AppHandle,
    state: State<'_, AppState>,
    preview_id: String,
    keys: Vec<String>,
    import_prefs: bool,
    import_recents: bool,
    import_favorites: bool,
) -> CmdResult<CommitOut> {
    let st = &*state;
    let gen = Gen::now(&st.lock_gen);
    let (results, patch, recents, favs) = {
        // lock order: vault, then import (same as import_preview)
        let mut g = guard(&st.vault);
        let v = g.as_mut().ok_or(LOCKED)?;
        let mut slot = st
            .accounts
            .import
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let pv = fresh(&mut slot, &preview_id, Instant::now(), gen).ok_or(EXPIRED)?;
        let picks: Vec<(String, &ImportedAccount)> = keys
            .iter()
            .filter_map(|k| Some((k.clone(), pv.accounts.get(k.parse::<usize>().ok()?)?)))
            .collect();
        let results = if picks.is_empty() {
            Vec::new()
        } else {
            commit_vault(v, &picks)?
        };
        let patch = pv
            .prefs
            .as_ref()
            .filter(|_| import_prefs)
            .map(prefs_patch)
            .filter(|p| *p != json!({}));
        let recents: Vec<Recent> = pv
            .recents
            .iter()
            .filter(|_| import_recents)
            .map(|g| Recent {
                place_id: g.place_id,
                name: game_name(g),
                thumbnail: cdn_only(&g.image_url),
                last_played_at: accounts::now_iso(),
            })
            .collect();
        let favs: Vec<Favorite> = pv
            .favorites
            .iter()
            .filter(|_| import_favorites)
            .map(|g| Favorite {
                id: accounts::new_id(),
                place_id: g.place_id,
                name: game_name(g),
                private_link: None,
                thumbnail: cdn_only(&g.image_url),
                order: 0,
            })
            .collect();
        (results, patch, recents, favs)
    };
    // Settings and library.json I/O after the vault and preview locks are released.
    let mut notes = Vec::new();
    let mut retry = false;
    if let Some(patch) = patch {
        notes.push(match persist(st, &patch) {
            // read-only mode: persist only applied them in memory
            Ok(_) if st.settings_read_only => {
                "Settings applied for this session only (settings.json is read-only).".into()
            }
            Ok(_) => "Settings imported.".into(),
            Err(e) => {
                retry = true;
                e
            }
        });
    }
    if !recents.is_empty() || !favs.is_empty() {
        let max = st
            .settings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pointer("/recents/max")
            .and_then(Value::as_u64)
            .unwrap_or(20) as usize;
        let path = st.paths.root.join("library.json");
        let nr = recents::import(&path, recents, max);
        let nf = favorites::add_all(&path, favs);
        if matches!(nr, Ok(n) if n > 0) {
            let _ = app.emit("recents://changed", Value::Null);
        }
        if matches!(nf, Ok(n) if n > 0) {
            let _ = app.emit("favorites://changed", Value::Null);
        }
        notes.push(match (nr, nf) {
            (Ok(nr), Ok(nf)) => format!("Games imported: {nr} recent, {nf} favorite."),
            (Err(e), _) | (_, Err(e)) => {
                retry = true;
                format!("Could not import games: {e}")
            }
        });
    }
    // Done: the decrypted set drops (zeroized). After a failure it stays (until its TTL or a
    // lock) so Import can run again without the old password; accounts come back as
    // updated/skipped and games dedupe.
    if !retry {
        let mut slot = st
            .accounts
            .import
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if slot.as_ref().is_some_and(|p| p.id == preview_id) {
            *slot = None;
        }
    }
    let ids: Vec<String> = results.iter().filter_map(|r| r.id.clone()).collect();
    if !ids.is_empty() {
        emit_changed(&app);
        accounts::sync::kick_headshots(&app);
        tauri::async_runtime::spawn(async move {
            // single flight: retry while another pass runs, up to 5 min or until a lock
            let st = app.state::<AppState>();
            for _ in 0..100 {
                if !gen.current(&st.lock_gen) {
                    break;
                }
                if let Some(f) = accounts::sync::Flight::claim(&app) {
                    accounts::sync::validate(&app, Some(ids.clone()), f).await;
                    break;
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        });
    }
    Ok(CommitOut {
        results,
        notes,
        retry,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::{LaunchTarget, WindowRect};
    use crate::vault::{oswrap::FakeOsWrap, store::CreateMode};
    use std::sync::Arc;

    fn legacy(uid: Option<u64>, name: &str) -> ImportedAccount {
        ImportedAccount {
            user_id: uid,
            username: name.into(),
            group: "Unsorted".into(),
            cookie: Some(Zeroizing::new(format!("COOKIE-SECRET-{name}"))),
            password: Some(Zeroizing::new(format!("PW-SECRET-{name}"))),
            ..Default::default()
        }
    }

    fn picks(list: &[ImportedAccount]) -> Vec<(String, &ImportedAccount)> {
        list.iter()
            .enumerate()
            .map(|(i, a)| (i.to_string(), a))
            .collect()
    }

    #[test]
    fn new_account_maps_every_field() {
        let mut a = legacy(Some(7), "Seven");
        a.alias = Some("Main".into());
        a.note = Some("n".into());
        a.group = "Alts".into();
        a.launch_target = Some(LaunchTarget {
            place_id: Some(606849621),
            job_id: Some("job".into()),
        });
        a.fps_cap = Some(240);
        a.window_rect = Some(WindowRect {
            x: -5,
            y: 10,
            w: 800,
            h: 600,
        });
        a.tracker_id = Some("987654321012".into());
        a.last_launched_at = Some("2024-01-02T03:04:05Z".into());
        a.extra.insert("k".into(), "v".into());
        let mut p = VaultPayload::default();
        let r = apply(&mut p, &picks(&[a]));
        assert_eq!(r[0].status, Status::Added);
        let x = &p.accounts[0];
        assert_eq!(x.user_id, Some(7));
        assert_eq!(x.group, "Alts");
        assert_eq!(x.alias.as_deref(), Some("Main"));
        assert_eq!(x.tracker_id, "987654321012");
        assert_eq!(x.fps_cap, Some(240));
        assert_eq!(x.launch_target.as_ref().unwrap().place_id, Some(606849621));
        assert_eq!(
            x.window_rect,
            Some(Rect {
                x: -5,
                y: 10,
                w: 800,
                h: 600
            })
        );
        assert_eq!(x.extra["k"], "v");
        assert_eq!(x.session, SessionState::Unknown);
        let s = &p.secrets[&x.id];
        assert_eq!(s.cookie.as_str(), "COOKIE-SECRET-Seven");
        assert_eq!(
            s.password.as_deref().map(|s| s.as_str()),
            Some("PW-SECRET-Seven")
        );
    }

    #[test]
    fn existing_keeps_metadata_and_fills_blanks() {
        let mut p = VaultPayload::default();
        let first = legacy(Some(7), "Seven");
        apply(&mut p, &picks(&[first]));
        let id = p.accounts[0].id.clone();
        let tracker = p.accounts[0].tracker_id.clone();
        p.accounts[0].alias = Some("Mine".into());
        p.accounts[0].group = "Keep".into();
        p.accounts[0].last_launched_at = Some("2026-01-01T00:00:00Z".into());
        p.accounts[0].session = SessionState::Expired;
        let mut again = legacy(Some(7), "Seven");
        again.alias = Some("Theirs".into());
        again.note = Some("from old".into());
        again.group = "Other".into();
        again.tracker_id = Some("111".into());
        again.cookie = Some(Zeroizing::new("NEW-COOKIE".into()));
        again.password = Some(Zeroizing::new("OLD-FILE-PW".into()));
        let r = apply(&mut p, &picks(&[again]));
        assert_eq!(r[0].status, Status::Updated);
        assert_eq!(p.accounts.len(), 1);
        let x = &p.accounts[0];
        assert_eq!(x.id, id);
        assert_eq!(x.alias.as_deref(), Some("Mine"));
        assert_eq!(x.note.as_deref(), Some("from old"));
        assert_eq!(x.group, "Keep");
        assert_eq!(x.tracker_id, tracker);
        assert_eq!(p.secrets[&id].cookie.as_str(), "NEW-COOKIE");
        // not signed in: cookie replaced, but RMI's non-empty password stays
        assert_eq!(
            p.secrets[&id].password.as_deref().map(|s| s.as_str()),
            Some("PW-SECRET-Seven")
        );
    }

    #[test]
    fn no_user_id_fails_and_duplicates_skip() {
        let mut p = VaultPayload::default();
        let list = [
            legacy(None, "A"),
            legacy(Some(1), "B"),
            legacy(Some(1), "B2"),
        ];
        let r = apply(&mut p, &picks(&list));
        let st: Vec<_> = r.iter().map(|r| r.status).collect();
        assert_eq!(st, [Status::Failed, Status::Added, Status::Skipped]);
        assert_eq!(p.accounts.len(), 1);
    }

    #[test]
    fn commit_is_one_save_with_a_backup() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("vault.rmiv");
        let os: Arc<dyn crate::vault::oswrap::OsWrap> = Arc::new(FakeOsWrap);
        let mut v = Vault::create(&path, CreateMode::Os, os.clone()).unwrap();
        let list: Vec<_> = (1..=5).map(|i| legacy(Some(i), &format!("U{i}"))).collect();
        let r = commit_vault(&mut v, &picks(&list)).unwrap();
        assert!(r.iter().all(|r| r.status == Status::Added));
        let backups = std::fs::read_dir(d.path().join("backups")).unwrap().count();
        assert_eq!(backups, 1, "one save, one required backup");
        let back = Vault::open(&path, None, os).unwrap();
        assert_eq!(back.payload().accounts.len(), 5);
    }

    #[test]
    fn preview_rows_never_carry_secrets() {
        let mut p = VaultPayload::default();
        apply(&mut p, &picks(&[legacy(Some(1), "Old")]));
        let list = [legacy(Some(1), "Old"), legacy(Some(2), "New")];
        let r = rows(&p, &list, |_| None);
        assert!(r[0].will_update && !r[1].will_update);
        assert!(r[0].has_cookie && r[0].has_password);
        let j = serde_json::to_string(&r).unwrap();
        assert!(!j.contains("SECRET"), "{j}");
    }

    #[test]
    fn expired_preview_is_dropped() {
        let at = Instant::now();
        let mut slot = Some(preview(at, 0));
        assert!(fresh(&mut slot, "y", at, g(0)).is_none());
        assert!(slot.is_some(), "wrong id keeps the preview");
        assert!(fresh(&mut slot, "x", at + TTL - Duration::from_secs(1), g(0)).is_some());
        assert!(fresh(&mut slot, "x", at + TTL, g(0)).is_none());
        assert!(
            slot.is_none(),
            "expired preview dropped (secrets zeroize on drop)"
        );
    }

    fn g(n: u64) -> Gen {
        Gen::now(&std::sync::atomic::AtomicU64::new(n))
    }

    fn preview(at: Instant, gen: u64) -> Preview {
        Preview {
            id: "x".into(),
            at,
            gen: g(gen),
            accounts: vec![legacy(Some(1), "A")],
            prefs: None,
            recents: vec![],
            favorites: vec![],
        }
    }

    #[test]
    fn game_names_and_thumbnails_are_sanitised() {
        let g = |name: &str| LegacyGame {
            place_id: 42,
            name: name.into(),
            image_url: None,
        };
        assert_eq!(game_name(&g("  ")), "42");
        assert_eq!(game_name(&g(&"x".repeat(300))).len(), 100);
        let c = |u: &str| cdn_only(&Some(u.into()));
        assert!(c("https://tr.rbxcdn.com/abc/150/150/Image/Png").is_some());
        assert!(c("http://tr.rbxcdn.com/a").is_none());
        assert!(c("https://evil.com/tr.rbxcdn.com").is_none());
        assert!(c("https://tr.rbxcdn.com.evil.com/a").is_none());
        assert!(c("https://x@tr.rbxcdn.com/a").is_none());
        assert!(c(r"https://evil.com\.rbxcdn.com/x").is_none());
        assert!(c("https://.rbxcdn.com/x").is_none());
        assert!(cdn_only(&None).is_none());
    }

    #[test]
    fn commit_after_lock_is_refused() {
        let at = Instant::now();
        let mut slot = Some(preview(at, 3));
        // a lock bumped the generation since the preview was made
        assert!(fresh(&mut slot, "x", at, g(4)).is_none());
        assert!(slot.is_none(), "stale preview dropped");
    }

    #[test]
    fn on_lock_clears_import_slot() {
        let rt = accounts::Runtime::default();
        *rt.import.lock().unwrap() = Some(preview(Instant::now(), 0));
        rt.on_lock();
        assert!(rt.import.lock().unwrap().is_none());
    }

    #[test]
    fn signed_in_account_keeps_its_cookie() {
        let mut p = VaultPayload::default();
        let mut first = legacy(Some(7), "Seven");
        first.password = None;
        apply(&mut p, &picks(&[first]));
        let id = p.accounts[0].id.clone();
        p.accounts[0].session = SessionState::Ok;
        let mut again = legacy(Some(7), "Seven");
        again.cookie = Some(Zeroizing::new("OLD-FILE-COOKIE".into()));
        let r = apply(&mut p, &picks(&[again]));
        assert_eq!(r[0].status, Status::Updated);
        assert_eq!(r[0].message.as_deref(), Some(KEPT));
        let s = &p.secrets[&id];
        assert_eq!(s.cookie.as_str(), "COOKIE-SECRET-Seven");
        // empty password filled from the old file
        assert_eq!(
            s.password.as_deref().map(|s| s.as_str()),
            Some("PW-SECRET-Seven")
        );
        assert_eq!(p.accounts[0].session, SessionState::Ok);
        // existing password is kept
        let mut third = legacy(Some(7), "Seven");
        third.password = Some(Zeroizing::new("OTHER".into()));
        apply(&mut p, &picks(&[third]));
        assert_eq!(
            p.secrets[&id].password.as_deref().map(|s| s.as_str()),
            Some("PW-SECRET-Seven")
        );
    }

    #[test]
    fn never_launched_existing_adopts_old_tracker_id() {
        let mut p = VaultPayload::default();
        apply(&mut p, &picks(&[legacy(Some(7), "Seven")]));
        let mut again = legacy(Some(7), "Seven");
        again.tracker_id = Some("987654321012".into());
        apply(&mut p, &picks(&[again]));
        assert_eq!(p.accounts[0].tracker_id, "987654321012");
    }

    #[test]
    fn duplicate_imported_tracker_id_is_not_copied() {
        let mut a = legacy(Some(7), "Seven");
        let mut b = legacy(Some(8), "Eight");
        a.tracker_id = Some("555".into());
        b.tracker_id = Some("555".into());
        let mut p = VaultPayload::default();
        apply(&mut p, &picks(&[a, b]));
        assert_eq!(p.accounts[0].tracker_id, "555");
        assert_ne!(p.accounts[1].tracker_id, "555");
    }

    #[test]
    fn prefs_map_to_settings_keys() {
        let v = prefs_patch(&LegacyPrefs {
            launch_delay_ms: Some(3000),
            fps_unlock: Some(true),
            fps_value: Some(240),
            multi_client: Some(false),
            recents_max: Some(30),
        });
        assert_eq!(
            v,
            json!({"launch":{"delayMs":3000},"fps":{"unlock":true,"value":240},
                   "multi":{"enabled":false},"recents":{"max":30}})
        );
        assert_eq!(prefs_patch(&LegacyPrefs::default()), json!({}));
        // the patch passes the settings type check
        crate::settings::apply_patch(&crate::settings::defaults(), &v).unwrap();
    }

    #[test]
    fn folder_must_be_an_absolute_directory() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("f.txt");
        std::fs::write(&file, b"x").unwrap();
        assert!(folder(&format!("  {}  ", d.path().display())).is_ok());
        assert!(folder("relative/dir").is_err());
        assert!(folder(&file.display().to_string()).is_err());
        assert!(folder(&d.path().join("missing").display().to_string()).is_err());
    }

    #[test]
    fn commit_without_any_user_id_saves_nothing() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("vault.rmiv");
        let os: Arc<dyn crate::vault::oswrap::OsWrap> = Arc::new(FakeOsWrap);
        let mut v = Vault::create(&path, CreateMode::Os, os).unwrap();
        let list = [legacy(None, "NoId")];
        let r = commit_vault(&mut v, &picks(&list)).unwrap();
        assert_eq!(r[0].status, Status::Failed);
        assert!(v.payload().accounts.is_empty());
    }

    #[test]
    fn bad_imported_tracker_ids_are_not_adopted() {
        for bad in ["12a", "", "123456789012345678901"] {
            let mut a = legacy(Some(7), "Seven");
            a.tracker_id = Some(bad.into());
            let mut p = VaultPayload::default();
            apply(&mut p, &picks(&[a]));
            let t = &p.accounts[0].tracker_id;
            assert!(
                t.len() == 12 && t.bytes().all(|b| b.is_ascii_digit()),
                "{bad:?} adopted as {t}"
            );
        }
    }

    #[test]
    fn existing_account_keeps_its_own_launch_target_and_window() {
        let mut p = VaultPayload::default();
        apply(&mut p, &picks(&[legacy(Some(7), "Seven")]));
        p.accounts[0].launch_target = Some(accounts::LaunchTarget {
            place_id: Some(1),
            job_id: None,
            private_link: None,
            mode: None,
        });
        p.accounts[0].window_rect = Some(Rect {
            x: 1,
            y: 1,
            w: 1,
            h: 1,
        });
        let mut again = legacy(Some(7), "Seven");
        again.launch_target = Some(LaunchTarget {
            place_id: Some(2),
            job_id: None,
        });
        again.window_rect = Some(WindowRect {
            x: 9,
            y: 9,
            w: 9,
            h: 9,
        });
        apply(&mut p, &picks(&[again]));
        let x = &p.accounts[0];
        assert_eq!(x.launch_target.as_ref().unwrap().place_id, Some(1));
        assert_eq!(
            x.window_rect,
            Some(Rect {
                x: 1,
                y: 1,
                w: 1,
                h: 1
            })
        );
    }

    #[test]
    fn window_rect_too_big_for_i32_is_dropped() {
        let mut a = legacy(Some(7), "Seven");
        a.window_rect = Some(WindowRect {
            x: 1 << 40,
            y: 0,
            w: 1,
            h: 1,
        });
        let mut p = VaultPayload::default();
        apply(&mut p, &picks(&[a]));
        assert_eq!(p.accounts[0].window_rect, None);
    }

    #[test]
    fn headshot_only_looked_up_for_ids() {
        let p = VaultPayload::default();
        let list = [legacy(Some(3), "A"), legacy(None, "B")];
        let r = rows(&p, &list, |u| Some(format!("h{u}")));
        assert_eq!(r[0].headshot.as_deref(), Some("h3"));
        assert_eq!(r[1].headshot, None);
    }

    #[test]
    fn existing_blank_launch_fields_filled_from_old() {
        let mut p = VaultPayload::default();
        apply(&mut p, &picks(&[legacy(Some(7), "Seven")]));
        let mut again = legacy(Some(7), "Seven");
        again.launch_target = Some(LaunchTarget {
            place_id: Some(5),
            job_id: None,
        });
        again.last_launched_at = Some("2024-01-02T03:04:05Z".into());
        again.extra.insert("k".into(), "v".into());
        apply(&mut p, &picks(&[again]));
        let x = &p.accounts[0];
        assert_eq!(x.launch_target.as_ref().unwrap().place_id, Some(5));
        assert_eq!(x.last_launched_at.as_deref(), Some("2024-01-02T03:04:05Z"));
        assert_eq!(x.extra["k"], "v");
    }
}

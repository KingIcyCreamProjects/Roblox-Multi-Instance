//! Accounts (spec §6.1): typed model stored in the vault payload, pure payload operations.
//! IPC lives in `cmds`, the sign-in webview in `signin`, background loops in `sync`.

pub mod cmds;
pub mod signin;
pub mod sync;

use crate::roblox::RobloxError;
use crate::vault::model::VaultPayload;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::{atomic::AtomicI64, Mutex};
use zeroize::Zeroizing;

pub const DEFAULT_GROUP: &str = "Unsorted";
const ALIAS_MAX: usize = 50;
const NOTE_MAX: usize = 5000;
const GROUP_MAX: usize = 64;
const GROUPS_MAX: usize = 200;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Ok,
    Expired,
    #[default]
    Unknown,
    /// Imported `username:password`, never signed in.
    Pending,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum PresenceState {
    Offline,
    Online,
    InGame,
    InStudio,
}

/// Runtime only (never stored in the vault).
#[derive(Serialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Presence {
    pub state: PresenceState,
    pub place_id: Option<u64>,
    pub universe_id: Option<u64>,
    pub game_name: Option<String>,
    pub job_id: Option<String>,
    pub updated_at: String,
}

impl Presence {
    /// Equal apart from `updated_at`.
    pub fn same(&self, o: &Presence) -> bool {
        Presence {
            updated_at: String::new(),
            ..self.clone()
        } == Presence {
            updated_at: String::new(),
            ..o.clone()
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct LaunchTarget {
    pub place_id: Option<u64>,
    pub job_id: Option<String>,
    pub private_link: Option<String>,
    /// How to pick a public server for `place_id` when no `job_id`: "random" or "smallest".
    pub mode: Option<crate::launch::resolve::Mode>,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Non-secret account fields. Unknown keys round-trip in `other`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Account {
    #[serde(default = "new_id")]
    pub id: String,
    pub user_id: Option<u64>,
    pub username: String,
    pub display_name: String,
    pub alias: Option<String>,
    pub note: Option<String>,
    #[serde(default = "default_group")]
    pub group: String,
    pub color: Option<u8>,
    pub pinned: bool,
    pub order: i64,
    #[serde(default = "now_iso")]
    pub created_at: String,
    pub last_launched_at: Option<String>,
    pub last_validated_at: Option<String>,
    pub session: SessionState,
    pub launch_target: Option<LaunchTarget>,
    pub fps_cap: Option<u32>,
    /// Roblox graphics level 1-10 this account starts at; `None` = the global setting.
    pub graphics: Option<u8>,
    pub window_rect: Option<Rect>,
    // ponytail: a missing trackerId is minted on load and persisted by the next save.
    #[serde(default = "new_tracker_id")]
    pub tracker_id: String,
    /// Friends may Disconnect/Reconnect this account through the friend remote.
    pub friend_allowed: bool,
    /// Backend only: a friend disconnected it; auto-reconnect must leave it alone.
    pub friend_paused: bool,
    /// Backend only: place the friend remote reconnects to.
    pub friend_last_place: Option<u64>,
    /// Backend only: the friend signed it in from the friend remote (Quick Log In).
    pub friend_added: bool,
    pub extra: BTreeMap<String, String>,
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

/// Rust-only. No Debug on purpose. Empty cookie = none (pending account).
#[derive(Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct AccountSecrets {
    pub cookie: Zeroizing<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<Zeroizing<String>>,
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

/// Best-effort zeroize of every string in `v`.
pub fn wipe_value(v: &mut Value) {
    use zeroize::Zeroize;
    match v {
        Value::String(s) => s.zeroize(),
        Value::Array(a) => a.iter_mut().for_each(wipe_value),
        Value::Object(m) => m.values_mut().for_each(wipe_value),
        _ => {}
    }
}

/// Parse `v`; if that fails, drop each unreadable key (logged, the field falls back to
/// its default) and retry. Only a non-object gives `None`. Keeps the vault readable.
pub fn lenient<T: DeserializeOwned>(v: &Value, what: &str) -> Option<T> {
    if let Ok(t) = T::deserialize(v) {
        return Some(t);
    }
    let Value::Object(m) = v else {
        log::warn!("{what}: unreadable entry dropped");
        return None;
    };
    let mut keep = Value::Object(Map::new());
    for (k, x) in m {
        let mut one = json!({ k: x });
        if T::deserialize(&one).is_ok() {
            keep[k] = x.clone();
        } else {
            log::warn!("{what}: field {k} unreadable, reset to default");
        }
        wipe_value(&mut one);
    }
    let r = T::deserialize(&keep).ok();
    wipe_value(&mut keep);
    r
}

fn default_group() -> String {
    DEFAULT_GROUP.into()
}

pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Random UUID v4.
pub fn new_id() -> String {
    let mut b: [u8; 16] = rand::random();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

/// Random 12-digit number string.
pub fn new_tracker_id() -> String {
    rand::random_range(100_000_000_000u64..1_000_000_000_000).to_string()
}

/// One pasted line → bare `.ROBLOSECURITY` value. Strips quotes, the `.ROBLOSECURITY=`
/// prefix, trailing `;` and whitespace; rejects anything that can't be a header value.
pub fn normalize_cookie(s: &str) -> Option<String> {
    const PREFIX: &str = ".ROBLOSECURITY=";
    let mut s = s;
    loop {
        let before = s;
        s = s.trim().trim_matches(['"', '\'']).trim_end_matches(';');
        if s.get(..PREFIX.len())
            .is_some_and(|p| p.eq_ignore_ascii_case(PREFIX))
        {
            s = &s[PREFIX.len()..];
        }
        if s == before {
            break;
        }
    }
    let ok = s.len() >= 100
        && s.bytes()
            .all(|b| b.is_ascii_graphic() && !b";,\"'\\".contains(&b));
    ok.then(|| s.to_owned())
}

/// `username:password` (Roblox usernames: 3-20 of `[A-Za-z0-9_]`).
pub fn parse_user_pass(line: &str) -> Option<(String, Zeroizing<String>)> {
    let (u, p) = line.trim().split_once(':')?;
    let u = u.trim();
    let ok = (3..=20).contains(&u.len())
        && u.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && !p.is_empty();
    ok.then(|| (u.to_owned(), Zeroizing::new(p.to_owned())))
}

/// Who a cookie (or username lookup) belongs to.
pub struct Identity {
    pub user_id: u64,
    pub username: String,
    pub display_name: String,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum AddStatus {
    Added,
    Updated,
    Invalid,
    Skipped,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AddResult {
    pub line: usize,
    pub status: AddStatus,
    pub username: Option<String>,
    pub message: Option<String>,
}

impl AddResult {
    pub fn invalid(line: usize, msg: impl Into<String>) -> Self {
        Self {
            line,
            status: AddStatus::Invalid,
            username: None,
            message: Some(msg.into()),
        }
    }
}

pub(crate) fn next_order(p: &VaultPayload, group: &str) -> i64 {
    p.accounts
        .iter()
        .filter(|a| a.group == group)
        .map(|a| a.order + 1)
        .max()
        .unwrap_or(0)
}

/// Add or update by `userId`. An update keeps all metadata and replaces the cookie (and
/// the password when given). Returns the status and the account id.
pub fn upsert(
    p: &mut VaultPayload,
    who: &Identity,
    cookie: Option<Zeroizing<String>>,
    password: Option<Zeroizing<String>>,
) -> (AddStatus, String) {
    let now = now_iso();
    let (status, i) = match p
        .accounts
        .iter()
        .position(|a| a.user_id == Some(who.user_id))
    {
        Some(i) => (AddStatus::Updated, i),
        None => {
            let a = Account {
                user_id: Some(who.user_id),
                order: next_order(p, DEFAULT_GROUP),
                session: SessionState::Pending,
                ..serde_json::from_value(json!({})).expect("defaults")
            };
            p.accounts.push(a);
            (AddStatus::Added, p.accounts.len() - 1)
        }
    };
    let a = &mut p.accounts[i];
    a.username.clone_from(&who.username);
    a.display_name.clone_from(&who.display_name);
    let id = a.id.clone();
    let fresh = cookie.is_some();
    if fresh {
        a.last_validated_at = Some(now);
    }
    let s = p.secrets.entry(id.clone()).or_default();
    if let Some(c) = cookie {
        s.cookie = c;
    }
    if password.is_some() {
        s.password = password;
    }
    if fresh {
        set_session(p, &id, SessionState::Ok, None);
    }
    (status, id)
}

/// Why `friend_upsert` did not add: `Refused` is plain words for the friend, `Internal`
/// never reaches him (log only).
#[derive(Debug, PartialEq)]
pub enum FriendUpsertError {
    Refused(String),
    Internal(String),
}

/// The friend signed `who` in from the friend remote: a new account is added through
/// `upsert`, shared with the friend and marked friend-added (at most `max` such); one
/// already shared just gets the new session. An owner-only account is never touched.
pub fn friend_upsert(
    p: &mut VaultPayload,
    who: &Identity,
    cookie: Zeroizing<String>,
    max: usize,
) -> Result<(), FriendUpsertError> {
    use FriendUpsertError::Refused;
    match p.accounts.iter().find(|a| a.user_id == Some(who.user_id)) {
        // Neutral: does not confirm the account is on the owner's computer.
        Some(a) if !a.friend_allowed => {
            return Err(Refused(
                "This account can't be added. Ask the owner.".into(),
            ))
        }
        None if p.accounts.iter().filter(|a| a.friend_added).count() >= max => {
            return Err(Refused(format!("You can add at most {max} accounts.")))
        }
        _ => {}
    }
    let (status, id) = upsert(p, who, Some(cookie), None);
    if status == AddStatus::Added {
        let i = find(p, &id).map_err(FriendUpsertError::Internal)?;
        p.accounts[i].friend_allowed = true;
        p.accounts[i].friend_added = true;
    }
    Ok(())
}

/// Sign-in result for an existing account (`id`) or, without one, a plain upsert.
pub fn signin_upsert(
    p: &mut VaultPayload,
    id: Option<&str>,
    who: &Identity,
    cookie: Zeroizing<String>,
    password: Option<Zeroizing<String>>,
) -> Result<(), String> {
    let Some(id) = id else {
        upsert(p, who, Some(cookie), password);
        return Ok(());
    };
    let a = p
        .accounts
        .iter()
        .find(|a| a.id == id)
        .ok_or("Account not found.")?;
    if a.user_id.is_some_and(|u| u != who.user_id) {
        return Err("Signed in as a different account.".into());
    }
    if p.accounts
        .iter()
        .any(|a| a.id != id && a.user_id == Some(who.user_id))
    {
        return Err("That Roblox account is already in your list.".into());
    }
    let a = p.accounts.iter_mut().find(|a| a.id == id).expect("found");
    a.user_id = Some(who.user_id);
    upsert(p, who, Some(cookie), password);
    Ok(())
}

const PATCHABLE: [&str; 10] = [
    "graphics",
    "alias",
    "note",
    "group",
    "color",
    "pinned",
    "launchTarget",
    "fpsCap",
    "friendAllowed",
    "extra",
];

/// Apply a camelCase patch (null clears). Validates before touching `a`.
pub fn apply_patch(a: &mut Account, patch: &Value) -> Result<(), String> {
    let Value::Object(m) = patch else {
        return Err("Invalid change.".into());
    };
    let mut v = serde_json::to_value(&*a).map_err(|_| "Invalid change.")?;
    for (k, x) in m {
        if !PATCHABLE.contains(&k.as_str()) {
            return Err(format!("{k} can't be changed."));
        }
        v[k] = match (k.as_str(), x) {
            ("group", Value::Null) => DEFAULT_GROUP.into(),
            ("pinned", Value::Null) => false.into(),
            ("extra", Value::Null) => json!({}),
            _ => x.clone(),
        };
    }
    let mut n: Account = serde_json::from_value(v).map_err(|_| "Invalid value.")?;
    n.alias = n
        .alias
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    n.note = n.note.filter(|s| !s.trim().is_empty());
    n.group = clean_group(&n.group)?;
    if n.alias
        .as_ref()
        .is_some_and(|s| s.chars().count() > ALIAS_MAX)
    {
        return Err(format!("Alias can be at most {ALIAS_MAX} characters."));
    }
    if n.note
        .as_ref()
        .is_some_and(|s| s.chars().count() > NOTE_MAX)
    {
        return Err(format!("Note can be at most {NOTE_MAX} characters."));
    }
    if n.color.is_some_and(|c| !(1..=8).contains(&c)) {
        return Err("Pick one of the 8 colors.".into());
    }
    // Only when changed: older accounts may hold a cap up to 1000 (the launch clamps it).
    if m.contains_key("fpsCap") && n.fps_cap.is_some_and(|f| !(1..=240).contains(&f)) {
        return Err("FPS cap must be between 1 and 240.".into());
    }
    if n.graphics.is_some_and(|g| !(1..=10).contains(&g)) {
        return Err("Graphics level must be between 1 and 10.".into());
    }
    // The owner took friend access away: a friend pause nobody can lift any more ends too.
    if !n.friend_allowed {
        n.friend_paused = false;
    }
    *a = n;
    Ok(())
}

pub(crate) fn clean_group(g: &str) -> Result<String, String> {
    let g = g.trim();
    if g.chars().count() > GROUP_MAX {
        return Err(format!("Group name can be at most {GROUP_MAX} characters."));
    }
    Ok(if g.is_empty() { DEFAULT_GROUP } else { g }.to_owned())
}

/// `update` + move to the end of a new group.
pub fn update(p: &mut VaultPayload, id: &str, patch: &Value) -> Result<(), String> {
    let i = find(p, id)?;
    let old = p.accounts[i].group.clone();
    let mut a = p.accounts[i].clone();
    apply_patch(&mut a, patch)?;
    let moved = a.group != old;
    if moved {
        a.order = next_order(p, &a.group);
    }
    p.accounts[i] = a; // an emptied group keeps its target (hidden by the view) until moved back
    Ok(())
}

pub fn find(p: &VaultPayload, id: &str) -> Result<usize, String> {
    p.accounts
        .iter()
        .position(|a| a.id == id)
        .ok_or_else(|| "Account not found.".into())
}

/// Account plus its secrets, for undo.
pub type Removed = (Account, Option<AccountSecrets>);

pub fn remove(p: &mut VaultPayload, ids: &[String]) -> Vec<Removed> {
    let (gone, keep) = std::mem::take(&mut p.accounts)
        .into_iter()
        .partition(|a| ids.contains(&a.id));
    p.accounts = keep; // no target prune: Undo must bring the group's target back
    gone.into_iter()
        .map(|a: Account| {
            let s = p.secrets.remove(&a.id);
            (a, s)
        })
        .collect()
}

pub fn restore(p: &mut VaultPayload, items: Vec<Removed>) {
    let mut renumber_later = Vec::new();
    for (a, s) in items {
        // re-added meanwhile (same id, or the same Roblox user): the newer entry wins
        if p.accounts
            .iter()
            .any(|x| x.id == a.id || (a.user_id.is_some() && x.user_id == a.user_id))
        {
            continue;
        }
        if let Some(s) = s {
            p.secrets.insert(a.id.clone(), s);
        }
        renumber_later.push(a.group.clone());
        p.accounts.push(a);
    }
    for g in renumber_later {
        renumber(p, &g);
    }
}

/// Orders in `group` become 0..n, keeping their current relative order.
fn renumber(p: &mut VaultPayload, group: &str) {
    let mut idx: Vec<usize> = (0..p.accounts.len())
        .filter(|&i| p.accounts[i].group == group)
        .collect();
    idx.sort_by_key(|&i| p.accounts[i].order);
    for (n, i) in idx.into_iter().enumerate() {
        p.accounts[i].order = n as i64;
    }
}

/// New group display order: trimmed, de-duplicated, bounded.
pub fn set_groups(p: &mut VaultPayload, groups: &[String]) -> Result<(), String> {
    if groups.len() > GROUPS_MAX {
        return Err(format!("At most {GROUPS_MAX} groups."));
    }
    let mut out: Vec<String> = Vec::new();
    for g in groups {
        let g = clean_group(g)?;
        if !out.contains(&g) {
            out.push(g);
        }
    }
    p.groups = out;
    Ok(())
}

/// `ids` = the full new order of `group`.
pub fn reorder(p: &mut VaultPayload, group: &str, ids: &[String]) -> Result<(), String> {
    let mut members: Vec<&String> = p
        .accounts
        .iter()
        .filter(|a| a.group == group)
        .map(|a| &a.id)
        .collect();
    let mut want: Vec<&String> = ids.iter().collect();
    members.sort();
    want.sort();
    if members != want {
        return Err("The list changed; refresh and try again.".into());
    }
    for (n, id) in ids.iter().enumerate() {
        let i = find(p, id)?;
        p.accounts[i].order = n as i64;
    }
    Ok(())
}

pub fn rename_group(p: &mut VaultPayload, from: &str, to: &str) -> Result<(), String> {
    let to = clean_group(to)?;
    if to == from {
        return Ok(());
    }
    // merging into an existing group appends after its accounts
    let offset = next_order(p, &to);
    for a in p.accounts.iter_mut().filter(|a| a.group == from) {
        a.group.clone_from(&to);
        a.order += offset;
    }
    for g in p.groups.iter_mut().filter(|g| *g == from) {
        g.clone_from(&to);
    }
    // the target follows the group; on a merge the existing group's target wins
    if let Some(t) = p.group_targets.remove(from) {
        p.group_targets.entry(to).or_insert(t);
    }
    let mut seen = std::collections::HashSet::new();
    p.groups.retain(|g| seen.insert(g.clone()));
    Ok(())
}

/// Save (`Some`) or clear (`None`) a group's target. Targets of emptied groups are pruned here.
pub fn set_group_target(
    p: &mut VaultPayload,
    group: &str,
    t: Option<LaunchTarget>,
) -> Result<(), String> {
    let g = clean_group(group)?;
    match t {
        Some(_) if !p.accounts.iter().any(|a| a.group == g) => {
            return Err("That group has no accounts.".into())
        }
        Some(t) => {
            p.group_targets.insert(g, t);
        }
        None => {
            p.group_targets.remove(&g);
        }
    }
    prune_group_targets(p);
    Ok(())
}

/// Drop targets of groups with no accounts (only on a target set; move/delete keep them for Undo).
fn prune_group_targets(p: &mut VaultPayload) {
    let used: std::collections::HashSet<&String> = p.accounts.iter().map(|a| &a.group).collect();
    p.group_targets.retain(|k, _| used.contains(k));
}

/// Used groups' targets only (a stale entry never shows or applies).
pub fn group_targets_view(p: &VaultPayload) -> BTreeMap<String, LaunchTarget> {
    p.group_targets
        .iter()
        .filter(|(g, _)| p.accounts.iter().any(|a| &a.group == *g))
        .map(|(g, t)| (g.clone(), t.clone()))
        .collect()
}

/// The account's own target, else its group's. `None` = use the Play bar.
/// A target with neither place nor link counts as unset (same rule as `effective_bar`).
pub fn target_for(p: &VaultPayload, a: &Account) -> Option<LaunchTarget> {
    let set = |t: &&LaunchTarget| t.place_id.is_some() || t.private_link.is_some();
    a.launch_target
        .as_ref()
        .filter(set)
        .or_else(|| p.group_targets.get(&a.group).filter(set))
        .cloned()
}

/// A friend disconnected this account: automation must not relaunch it.
pub fn friend_paused(p: &VaultPayload, id: &str) -> bool {
    p.accounts.iter().any(|a| a.id == id && a.friend_paused)
}

/// Display order: stored order first, then any used group missing from it. Unused dropped.
pub fn groups_view(p: &VaultPayload) -> Vec<String> {
    let used = |g: &String| p.accounts.iter().any(|a| &a.group == g);
    let mut out: Vec<String> = Vec::new();
    for g in p.groups.iter().chain(p.accounts.iter().map(|a| &a.group)) {
        if used(g) && !out.contains(g) {
            out.push(g.clone());
        }
    }
    out
}

/// Contract `AccountView`: the account minus unknown keys, plus runtime fields.
pub fn view(
    a: &Account,
    s: Option<&AccountSecrets>,
    presence: Option<&Presence>,
    headshot: Option<String>,
) -> Value {
    let mut v = serde_json::to_value(a).expect("account serializes");
    let o = v.as_object_mut().expect("object");
    for k in a.other.keys() {
        o.remove(k);
    }
    o.insert("presence".into(), json!(presence));
    o.insert(
        "hasPassword".into(),
        json!(s.is_some_and(|s| s.password.is_some())),
    );
    o.insert(
        "hasCookie".into(),
        json!(s.is_some_and(|s| !s.cookie.is_empty())),
    );
    o.insert("headshot".into(), json!(headshot));
    v
}

pub fn map_presence(
    p: &crate::roblox::api::presence::Presence,
    game_name: Option<String>,
    now: &str,
) -> Presence {
    Presence {
        state: match p.user_presence_type {
            1 => PresenceState::Online,
            2 => PresenceState::InGame,
            3 => PresenceState::InStudio,
            _ => PresenceState::Offline,
        },
        place_id: p.place_id,
        universe_id: p.universe_id,
        game_name,
        job_id: p.game_id.clone(),
        updated_at: now.to_owned(),
    }
}

/// The one session transition: every writer of `Account::session` goes through here
/// (`Pending` is only ever the state a new account starts in). `about` = the cookie the
/// verdict was reached with: if the stored cookie differs (re-sign-in or paste meanwhile)
/// nothing changes. `None` = the caller just stored the cookie. Returns whether it changed.
pub fn set_session(p: &mut VaultPayload, id: &str, to: SessionState, about: Option<&str>) -> bool {
    let current = |c: &str| p.secrets.get(id).is_some_and(|s| s.cookie.as_str() == c);
    if to == SessionState::Pending || about.is_some_and(|c| !current(c)) {
        return false;
    }
    match p.accounts.iter_mut().find(|a| a.id == id) {
        Some(a) if a.session != to => {
            a.session = to;
            true
        }
        _ => false,
    }
}

/// `users::authenticated` result → session state.
pub fn session_of<T>(r: &crate::roblox::Result<T>) -> SessionState {
    match r {
        Ok(_) => SessionState::Ok,
        Err(RobloxError::Unauthorized) => SessionState::Expired,
        Err(_) => SessionState::Unknown,
    }
}

/// In-memory account state that never goes to disk. Cleared on lock.
#[derive(Default)]
pub struct Runtime {
    pub presence: Mutex<HashMap<String, Presence>>,
    /// undoToken → (hold seq, removed accounts with secrets), dropped after `cmds::UNDO_SECS`.
    /// The seq lets a re-hold outlive the first hold's timer.
    pub removed: Mutex<HashMap<String, (u64, Vec<Removed>)>>,
    /// Unix secs until which secret copy/reveal is allowed.
    pub reauth_until: AtomicI64,
    /// universeId → (name, fetched).
    pub games: Mutex<HashMap<u64, (String, std::time::Instant)>>,
    pub clipboard: Mutex<Option<arboard::Clipboard>>,
    /// Bumped on every copy; a pending 30 s clear only runs if no newer copy happened.
    pub clip_seq: std::sync::atomic::AtomicU64,
    /// The last copied secret while it may still be on the clipboard (cleared on lock too).
    pub clip_secret: Mutex<Option<Zeroizing<String>>>,
    /// Single-flight guard for session validation.
    pub validating: std::sync::atomic::AtomicBool,
    /// Decrypted legacy import awaiting commit (secrets zeroize on drop).
    pub import: Mutex<Option<crate::import::cmds::Preview>>,
}

impl Runtime {
    pub fn on_lock(&self) {
        use std::sync::PoisonError;
        self.presence
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        self.removed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        *self.import.lock().unwrap_or_else(PoisonError::into_inner) = None;
        self.reauth_until
            .store(0, std::sync::atomic::Ordering::SeqCst);
        // a copied secret must not outlive the lock; the pending 30 s clear becomes a no-op
        self.clip_seq
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        cmds::clear_secret_clip(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn cookie(tag: &str) -> String {
        format!("_|WARNING:-DO-NOT-SHARE-THIS.--Sharing-this-will-allow-someone-to-log-in-as-you-and-to-steal-your-ROBUX-and-items.|_{tag}ABCDEF0123456789")
    }

    fn who(id: u64, name: &str) -> Identity {
        Identity {
            user_id: id,
            username: name.into(),
            display_name: format!("{name} D"),
        }
    }

    #[test]
    fn normalize_cookie_variants() {
        let c = cookie("x");
        for raw in [
            c.clone(),
            format!("  {c}  "),
            format!("\"{c}\""),
            format!("'{c}'"),
            format!(".ROBLOSECURITY={c}"),
            format!(".roblosecurity={c};"),
            format!("{c};"),
            format!("\".ROBLOSECURITY={c};\""),
            format!(".ROBLOSECURITY=\"{c}\";\r"),
            format!("\t{c}\r\n"),
        ] {
            assert_eq!(
                normalize_cookie(&raw).as_deref(),
                Some(c.as_str()),
                "{raw:?}"
            );
        }
        for bad in [
            "",
            "   ",
            "hello",
            "user:pass",
            &format!("{c} trailing"),
            &format!("{c};other=1"),
        ] {
            assert_eq!(normalize_cookie(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn parse_user_pass_rules() {
        let (u, p) = parse_user_pass(" alt_01:pa:ss ").unwrap();
        assert_eq!((u.as_str(), p.as_str()), ("alt_01", "pa:ss"));
        for bad in ["ab:x", "nouser", "bad name:x", "okname:", "a-b-c:x"] {
            assert!(parse_user_pass(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn dedupe_keeps_metadata_replaces_cookie() {
        let mut p = VaultPayload::default();
        let (s, id) = upsert(
            &mut p,
            &who(7, "a"),
            Some(Zeroizing::new("c1".into())),
            None,
        );
        assert_eq!(s, AddStatus::Added);
        update(
            &mut p,
            &id,
            &json!({"alias": "Main", "note": "n", "group": "Farm"}),
        )
        .unwrap();
        let (s, id2) = upsert(
            &mut p,
            &who(7, "a2"),
            Some(Zeroizing::new("c2".into())),
            Some(Zeroizing::new("pw".into())),
        );
        assert_eq!((s, id2.as_str()), (AddStatus::Updated, id.as_str()));
        assert_eq!(p.accounts.len(), 1);
        let a = &p.accounts[0];
        assert_eq!(
            (a.alias.as_deref(), a.note.as_deref(), a.group.as_str()),
            (Some("Main"), Some("n"), "Farm")
        );
        assert_eq!(a.username, "a2");
        assert_eq!(a.session, SessionState::Ok);
        assert_eq!(p.secrets[&id].cookie.as_str(), "c2");
        assert_eq!(
            p.secrets[&id].password.as_deref().map(|s| s.as_str()),
            Some("pw")
        );
        assert_eq!(a.tracker_id.len(), 12);
        assert!(a.tracker_id.bytes().all(|b| b.is_ascii_digit()));
        // cookie-less update (pending import) keeps the cookie
        upsert(
            &mut p,
            &who(7, "a2"),
            None,
            Some(Zeroizing::new("pw2".into())),
        );
        assert_eq!(p.secrets[&id].cookie.as_str(), "c2");
    }

    #[test]
    fn patch_validation() {
        let mut a: Account = serde_json::from_value(json!({})).unwrap();
        assert!(apply_patch(&mut a, &json!({"alias": "x".repeat(50)})).is_ok());
        let e = apply_patch(&mut a, &json!({"alias": "x".repeat(51)})).unwrap_err();
        assert_eq!(e, "Alias can be at most 50 characters.");
        assert_eq!(a.alias.as_ref().unwrap().len(), 50, "unchanged on error");
        assert!(apply_patch(&mut a, &json!({"note": "y".repeat(5001)})).is_err());
        assert!(apply_patch(&mut a, &json!({"color": 9})).is_err());
        assert!(apply_patch(&mut a, &json!({"fpsCap": 0})).is_err());
        assert!(apply_patch(&mut a, &json!({"fpsCap": 241})).is_err());
        assert!(apply_patch(&mut a, &json!({"graphics": 11})).is_err());
        // an old cap above 240 doesn't block other edits
        a.fps_cap = Some(1000);
        assert!(apply_patch(&mut a, &json!({"note": "ok"})).is_ok());
        a.fps_cap = None;
        assert!(apply_patch(&mut a, &json!({"username": "hax"})).is_err());
        assert!(apply_patch(&mut a, &json!({"pinned": "yes"})).is_err());
        apply_patch(
            &mut a,
            &json!({"alias": null, "group": null, "color": 3, "launchTarget": {"placeId": 1}}),
        )
        .unwrap();
        assert_eq!(
            (a.alias.as_deref(), a.group.as_str(), a.color),
            (None, "Unsorted", Some(3))
        );
        assert_eq!(a.launch_target.as_ref().unwrap().place_id, Some(1));
    }

    #[test]
    fn remove_then_restore() {
        let mut p = VaultPayload::default();
        let (_, a) = upsert(&mut p, &who(1, "a"), Some(Zeroizing::new("c".into())), None);
        upsert(&mut p, &who(2, "b"), None, None);
        let before = p.clone();
        let gone = remove(&mut p, std::slice::from_ref(&a));
        assert_eq!(p.accounts.len(), 1);
        assert!(!p.secrets.contains_key(&a));
        restore(&mut p, gone);
        assert_eq!(p.accounts.len(), 2);
        assert!(p.secrets[&a].cookie.as_str() == "c");
        assert!(before.accounts.iter().all(|x| p.accounts.contains(x)));
        // the same Roblox user re-added before Undo: no second row for that userId
        let gone = remove(&mut p, std::slice::from_ref(&a));
        upsert(
            &mut p,
            &who(1, "a"),
            Some(Zeroizing::new("new".into())),
            None,
        );
        restore(&mut p, gone);
        assert_eq!(p.accounts.len(), 2);
        assert_eq!(
            p.secrets
                .values()
                .filter(|s| s.cookie.as_str() == "new")
                .count(),
            1
        );
    }

    #[test]
    fn groups_and_reorder() {
        let mut p = VaultPayload::default();
        let ids: Vec<_> = (1..=3)
            .map(|i| upsert(&mut p, &who(i, "u"), None, None).1)
            .collect();
        assert_eq!(
            p.accounts.iter().map(|a| a.order).collect::<Vec<_>>(),
            [0, 1, 2]
        );
        reorder(
            &mut p,
            DEFAULT_GROUP,
            &[ids[2].clone(), ids[0].clone(), ids[1].clone()],
        )
        .unwrap();
        assert_eq!(
            p.accounts.iter().map(|a| a.order).collect::<Vec<_>>(),
            [1, 2, 0]
        );
        update(&mut p, &ids[0], &json!({"group": " Farm "})).unwrap();
        p.groups = vec!["Gone".into(), "Farm".into()];
        assert_eq!(groups_view(&p), ["Farm", "Unsorted"]);
        rename_group(&mut p, "Farm", "Unsorted").unwrap();
        assert_eq!(groups_view(&p), ["Unsorted"]);
        assert!(reorder(&mut p, "Nope", &[ids[0].clone()]).is_err());
    }

    #[test]
    fn group_targets_resolve_rename_clear() {
        let mut p = VaultPayload::default();
        let ids: Vec<_> = (1..=2)
            .map(|i| upsert(&mut p, &who(i, "u"), None, None).1)
            .collect();
        update(&mut p, &ids[0], &json!({"group": "RC"})).unwrap();
        update(&mut p, &ids[1], &json!({"group": "RC"})).unwrap();
        let link = |s: &str| LaunchTarget {
            private_link: Some(s.into()),
            ..Default::default()
        };
        assert!(set_group_target(&mut p, "Nope", Some(link("x"))).is_err());
        set_group_target(&mut p, " RC ", Some(link("g"))).unwrap();
        update(&mut p, &ids[1], &json!({"launchTarget": {"placeId": 7}})).unwrap();
        // own target, else group target
        assert_eq!(target_for(&p, &p.accounts[0]), Some(link("g")));
        assert_eq!(target_for(&p, &p.accounts[1]).unwrap().place_id, Some(7));
        rename_group(&mut p, "RC", "Main").unwrap();
        assert_eq!(group_targets_view(&p).keys().collect::<Vec<_>>(), ["Main"]);
        assert_eq!(target_for(&p, &p.accounts[0]), Some(link("g")));
        set_group_target(&mut p, "Main", None).unwrap();
        assert_eq!(target_for(&p, &p.accounts[0]), None);
        // emptied group by a move: hidden, comes back when an account moves back
        set_group_target(&mut p, "Main", Some(link("g"))).unwrap();
        update(&mut p, &ids[0], &json!({"group": null})).unwrap();
        update(&mut p, &ids[1], &json!({"group": null})).unwrap();
        assert!(group_targets_view(&p).is_empty());
        update(&mut p, &ids[0], &json!({"group": "Main"})).unwrap();
        assert_eq!(target_for(&p, &p.accounts[0]), Some(link("g")));
        update(&mut p, &ids[0], &json!({"group": null})).unwrap();
        // the next target set prunes it
        set_group_target(&mut p, "Unsorted", Some(link("u"))).unwrap();
        assert_eq!(p.group_targets.keys().collect::<Vec<_>>(), ["Unsorted"]);
        let back: VaultPayload = serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
        assert_eq!(back.group_targets, p.group_targets);
        // emptied group by a delete: Undo brings the target back
        let gone = remove(&mut p, &ids);
        assert!(group_targets_view(&p).is_empty());
        restore(&mut p, gone);
        assert_eq!(
            group_targets_view(&p).keys().collect::<Vec<_>>(),
            ["Unsorted"]
        );
    }

    #[test]
    fn signin_target_rules() {
        let mut p = VaultPayload::default();
        let (_, pend) = upsert(&mut p, &who(5, "alt"), None, None);
        let c = || Zeroizing::new("c".to_string());
        assert_eq!(
            signin_upsert(&mut p, Some(&pend), &who(6, "other"), c(), None).unwrap_err(),
            "Signed in as a different account."
        );
        signin_upsert(&mut p, Some(&pend), &who(5, "alt"), c(), Some(c())).unwrap();
        assert!(p.secrets[&pend].password.is_some());
        assert_eq!(p.accounts[0].session, SessionState::Ok);
        assert_eq!(p.accounts.len(), 1);
        signin_upsert(&mut p, None, &who(6, "other"), c(), None).unwrap();
        assert_eq!(p.accounts.len(), 2);
    }

    #[test]
    fn payload_migrates_generic_json_losslessly() {
        let raw = json!({
            "accounts": [{"id": "a1", "userId": 9, "username": "u", "legacyField": {"k": [1]}}],
            "secrets": {"a1": {"cookie": "c", "password": "p"}},
            "future": 42
        });
        let p: VaultPayload = serde_json::from_value(raw).unwrap();
        let a = &p.accounts[0];
        assert_eq!(
            (a.id.as_str(), a.user_id, a.group.as_str()),
            ("a1", Some(9), "Unsorted")
        );
        assert_eq!(a.tracker_id.len(), 12);
        let back = serde_json::to_value(&p).unwrap();
        assert_eq!(back["accounts"][0]["legacyField"], json!({"k": [1]}));
        assert_eq!(back["secrets"]["a1"]["password"], "p");
        assert_eq!(back["future"], 42);
        // view never leaks unknown keys or secrets
        let v = view(a, p.secrets.get("a1"), None, None);
        assert!(v.get("legacyField").is_none());
        assert_eq!(
            (v["hasCookie"].clone(), v["hasPassword"].clone()),
            (json!(true), json!(true))
        );
        assert!(!v.to_string().contains("\"c\"") && !v.to_string().contains("\"p\""));
        assert_eq!(v["session"], "unknown");
    }

    #[test]
    fn presence_and_session_mapping() {
        use crate::roblox::api::presence::Presence as P;
        let p = |t| P {
            user_id: 1,
            user_presence_type: t,
            last_location: None,
            place_id: Some(10),
            root_place_id: None,
            game_id: Some("job".into()),
            universe_id: Some(20),
        };
        let states: Vec<_> = (0..5)
            .map(|t| map_presence(&p(t), None, "t").state)
            .collect();
        use PresenceState::*;
        assert_eq!(states, [Offline, Online, InGame, InStudio, Offline]);
        let m = map_presence(&p(2), Some("G".into()), "t");
        assert_eq!(
            (
                m.place_id,
                m.universe_id,
                m.job_id.as_deref(),
                m.game_name.as_deref()
            ),
            (Some(10), Some(20), Some("job"), Some("G"))
        );
        assert!(m.same(&map_presence(&p(2), Some("G".into()), "later")));
        assert_eq!(serde_json::to_value(InGame).unwrap(), "in_game");
        assert_eq!(session_of::<()>(&Ok(())), SessionState::Ok);
        assert_eq!(
            session_of::<()>(&Err(RobloxError::Unauthorized)),
            SessionState::Expired
        );
        assert_eq!(
            session_of::<()>(&Err(RobloxError::Network("x".into()))),
            SessionState::Unknown
        );
    }

    #[test]
    fn set_session_drops_stale_verdicts_and_never_goes_pending() {
        let mut p = VaultPayload::default();
        let c = Zeroizing::new(cookie("a"));
        let (_, id) = upsert(&mut p, &who(1, "a"), Some(c.clone()), None);
        assert_eq!(p.accounts[0].session, SessionState::Ok);
        let state = |p: &VaultPayload| p.accounts[0].session;
        // a 401 about an older cookie: ignored
        assert!(!set_session(
            &mut p,
            &id,
            SessionState::Expired,
            Some("old")
        ));
        assert_eq!(state(&p), SessionState::Ok);
        assert!(set_session(&mut p, &id, SessionState::Expired, Some(&c)));
        assert!(
            !set_session(&mut p, &id, SessionState::Expired, Some(&c)),
            "unchanged"
        );
        assert!(!set_session(&mut p, &id, SessionState::Pending, None));
        assert!(!set_session(&mut p, "nope", SessionState::Ok, None));
        assert_eq!(state(&p), SessionState::Expired);
    }

    #[test]
    fn bad_fields_reset_not_damaged() {
        let raw = json!({
            "accounts": [
                {"id": "a1", "alias": "A", "color": "red", "pinned": 3, "userId": -1, "x": 1},
                "garbage"
            ],
            "secrets": {"a1": {"cookie": 5, "password": "p", "keep": "k"}},
            "groups": "nope"
        });
        let p: VaultPayload = serde_json::from_value(raw).unwrap();
        assert_eq!(p.accounts.len(), 1);
        let a = &p.accounts[0];
        assert_eq!((a.id.as_str(), a.alias.as_deref()), ("a1", Some("A")));
        assert_eq!((a.color, a.pinned, a.user_id), (None, false, None));
        assert_eq!(a.other["x"], 1);
        let s = &p.secrets["a1"];
        assert!(s.cookie.is_empty());
        assert_eq!(s.password.as_deref().map(|s| s.as_str()), Some("p"));
        assert_eq!(s.other["keep"], "k");
        assert!(p.groups.is_empty());
    }

    #[test]
    fn reorder_needs_full_set_and_restore_renumbers() {
        let mut p = VaultPayload::default();
        let ids: Vec<_> = (1..=3)
            .map(|i| upsert(&mut p, &who(i, "u"), None, None).1)
            .collect();
        assert!(
            reorder(&mut p, DEFAULT_GROUP, &ids[..2]).is_err(),
            "partial"
        );
        let dup = [ids[0].clone(), ids[0].clone(), ids[1].clone()];
        assert!(reorder(&mut p, DEFAULT_GROUP, &dup).is_err(), "dupe");
        let gone = remove(&mut p, &ids[..1]);
        reorder(&mut p, DEFAULT_GROUP, &[ids[2].clone(), ids[1].clone()]).unwrap();
        restore(&mut p, gone);
        let mut orders: Vec<_> = p.accounts.iter().map(|a| a.order).collect();
        orders.sort();
        assert_eq!(orders, [0, 1, 2]);
        set_groups(&mut p, &[" A ".into(), "A".into(), "".into()]).unwrap();
        assert_eq!(p.groups, ["A", "Unsorted"]);
        assert!(set_groups(&mut p, &vec!["g".to_string(); 201]).is_err());
        assert!(set_groups(&mut p, &["x".repeat(65)]).is_err());
    }

    #[test]
    fn ids_look_right() {
        let id = new_id();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4");
        assert_ne!(new_id(), id);
    }

    #[test]
    fn cookie_length_and_charset_edges() {
        assert_eq!(normalize_cookie(&"a".repeat(99)), None, "too short");
        assert_eq!(
            normalize_cookie(&"a".repeat(100)).as_deref(),
            Some("a".repeat(100).as_str())
        );
        assert_eq!(normalize_cookie(&format!("{}\\b", "a".repeat(100))), None);
        assert_eq!(normalize_cookie(&format!("{}é", "a".repeat(100))), None);
        // multibyte text where the prefix check would slice mid-char: no panic
        assert_eq!(normalize_cookie(&"é".repeat(60)), None);
    }

    #[test]
    fn user_pass_length_bounds() {
        let ok = |n: usize| parse_user_pass(&format!("{}:pw", "u".repeat(n))).is_some();
        assert!(!ok(2));
        assert!(ok(3));
        assert!(ok(20));
        assert!(!ok(21));
        assert_eq!(parse_user_pass(" abc :pw").unwrap().0, "abc");
    }

    #[test]
    fn pending_import_then_cookie_sign_in() {
        let mut p = VaultPayload::default();
        let (s, id) = upsert(
            &mut p,
            &who(4, "p"),
            None,
            Some(Zeroizing::new("pw".into())),
        );
        assert_eq!(s, AddStatus::Added);
        assert_eq!(p.accounts[0].session, SessionState::Pending);
        assert_eq!(p.accounts[0].last_validated_at, None);
        assert_eq!(p.secrets[&id].cookie.as_str(), "");
        // a cookie arriving later makes it Ok and stamps the check time
        upsert(
            &mut p,
            &who(4, "p"),
            Some(Zeroizing::new(cookie("z"))),
            None,
        );
        assert_eq!(p.accounts[0].session, SessionState::Ok);
        assert!(p.accounts[0].last_validated_at.is_some());
    }

    #[test]
    fn signin_refuses_unknown_and_duplicate() {
        let mut p = VaultPayload::default();
        let c = || Zeroizing::new("c".to_string());
        upsert(&mut p, &who(6, "two"), Some(c()), None);
        // legacy row with no user id yet
        p.accounts
            .push(serde_json::from_value(json!({"id": "x", "username": "old"})).unwrap());
        assert_eq!(
            signin_upsert(&mut p, Some("missing"), &who(5, "alt"), c(), None).unwrap_err(),
            "Account not found."
        );
        assert_eq!(
            signin_upsert(&mut p, Some("x"), &who(6, "two"), c(), None).unwrap_err(),
            "That Roblox account is already in your list."
        );
        assert_eq!(p.accounts[1].user_id, None, "untouched on error");
        signin_upsert(&mut p, Some("x"), &who(7, "new"), c(), None).unwrap();
        assert_eq!((p.accounts.len(), p.accounts[1].user_id), (2, Some(7)));
    }

    #[test]
    fn patch_error_messages_and_blank_clears() {
        let mut a: Account = serde_json::from_value(json!({})).unwrap();
        assert_eq!(
            apply_patch(&mut a, &json!("x")).unwrap_err(),
            "Invalid change."
        );
        assert_eq!(
            apply_patch(&mut a, &json!({"id": "y"})).unwrap_err(),
            "id can't be changed."
        );
        assert_eq!(
            apply_patch(&mut a, &json!({"group": "g".repeat(65)})).unwrap_err(),
            "Group name can be at most 64 characters."
        );
        assert_eq!(
            apply_patch(&mut a, &json!({"color": 0})).unwrap_err(),
            "Pick one of the 8 colors."
        );
        assert_eq!(
            apply_patch(&mut a, &json!({"graphics": 0})).unwrap_err(),
            "Graphics level must be between 1 and 10."
        );
        apply_patch(
            &mut a,
            &json!({"alias": "   ", "note": " \n ", "group": "  "}),
        )
        .unwrap();
        assert_eq!(
            (a.alias.as_deref(), a.note.as_deref(), a.group.as_str()),
            (None, None, "Unsorted")
        );
    }

    #[test]
    fn rename_merge_appends_and_keeps_existing_target() {
        let mut p = VaultPayload::default();
        let ids: Vec<_> = (1..=3)
            .map(|i| upsert(&mut p, &who(i, "u"), None, None).1)
            .collect();
        update(&mut p, &ids[0], &json!({"group": "A"})).unwrap();
        update(&mut p, &ids[1], &json!({"group": "A"})).unwrap();
        update(&mut p, &ids[2], &json!({"group": "B"})).unwrap();
        let t = |s: &str| LaunchTarget {
            private_link: Some(s.into()),
            ..Default::default()
        };
        set_group_target(&mut p, "A", Some(t("a"))).unwrap();
        set_group_target(&mut p, "B", Some(t("b"))).unwrap();
        p.groups = vec!["A".into(), "B".into()];
        rename_group(&mut p, "A", "B").unwrap();
        let got: Vec<_> = p
            .accounts
            .iter()
            .map(|a| (a.group.as_str(), a.order))
            .collect();
        // merged accounts go after the existing group's accounts
        assert_eq!(got, [("B", 1), ("B", 2), ("B", 0)]);
        assert_eq!(
            p.group_targets.get("B"),
            Some(&t("b")),
            "existing target wins"
        );
        assert!(!p.group_targets.contains_key("A"));
        assert_eq!(p.groups, ["B"], "no duplicate group names");
    }

    #[test]
    fn unknown_ids_error_without_change() {
        let mut p = VaultPayload::default();
        let (_, id) = upsert(&mut p, &who(1, "a"), None, None);
        let before = p.clone();
        assert_eq!(find(&p, "nope").unwrap_err(), "Account not found.");
        assert_eq!(
            update(&mut p, "nope", &json!({"alias": "x"})).unwrap_err(),
            "Account not found."
        );
        assert!(remove(&mut p, &["nope".to_string()]).is_empty());
        assert!(reorder(&mut p, DEFAULT_GROUP, &[id, "nope".into()]).is_err());
        assert!(p == before, "unchanged");
    }

    #[test]
    fn lenient_drops_only_bad_fields() {
        let a: Account = lenient(&json!({"id": "a", "alias": "A", "color": "red"}), "t").unwrap();
        assert_eq!(
            (a.id.as_str(), a.alias.as_deref(), a.color),
            ("a", Some("A"), None)
        );
        // no default to fall back to: nothing salvageable
        assert!(lenient::<Rect>(&json!({"x": 1, "y": 2, "w": "a", "h": 4}), "t").is_none());
        assert!(lenient::<Rect>(&json!("garbage"), "t").is_none());
    }

    #[test]
    fn wipe_zeroes_strings_only() {
        let mut v = json!({"a": "secret", "b": ["x", 1, {"c": "y"}], "n": 5});
        wipe_value(&mut v);
        assert_eq!(v, json!({"a": "", "b": ["", 1, {"c": ""}], "n": 5}));
    }

    #[test]
    fn presence_same_compares_state_not_time() {
        let base = Presence {
            state: PresenceState::Online,
            place_id: None,
            universe_id: None,
            game_name: None,
            job_id: None,
            updated_at: "a".into(),
        };
        let later = Presence {
            updated_at: "b".into(),
            ..base.clone()
        };
        assert!(base.same(&later));
        assert!(!base.same(&Presence {
            state: PresenceState::InGame,
            ..later.clone()
        }));
        assert!(!base.same(&Presence {
            job_id: Some("j".into()),
            ..later
        }));
    }

    #[test]
    fn lock_clears_runtime_state() {
        use std::sync::atomic::Ordering::SeqCst;
        let rt = Runtime::default();
        rt.presence.lock().unwrap().insert(
            "a".into(),
            Presence {
                state: PresenceState::Online,
                place_id: None,
                universe_id: None,
                game_name: None,
                job_id: None,
                updated_at: "t".into(),
            },
        );
        rt.removed
            .lock()
            .unwrap()
            .insert("tok".into(), (0, Vec::new()));
        rt.reauth_until.store(i64::MAX, SeqCst);
        rt.on_lock();
        assert!(rt.presence.lock().unwrap().is_empty());
        assert!(rt.removed.lock().unwrap().is_empty());
        assert_eq!(rt.reauth_until.load(SeqCst), 0);
        assert_eq!(
            rt.clip_seq.load(SeqCst),
            1,
            "pending clipboard clear is voided"
        );
    }

    #[test]
    fn upsert_without_password_keeps_saved_one() {
        let mut p = VaultPayload::default();
        let (_, id) = upsert(
            &mut p,
            &who(8, "k"),
            Some(Zeroizing::new(cookie("a"))),
            Some(Zeroizing::new("pw".into())),
        );
        upsert(
            &mut p,
            &who(8, "k"),
            Some(Zeroizing::new(cookie("b"))),
            None,
        );
        assert_eq!(p.secrets[&id].cookie.as_str(), cookie("b"));
        assert_eq!(
            p.secrets[&id].password.as_deref().map(|s| s.as_str()),
            Some("pw")
        );
    }

    #[test]
    fn patch_null_resets_pinned_and_extra() {
        let mut a: Account =
            serde_json::from_value(json!({"pinned": true, "extra": {"k": "v"}})).unwrap();
        apply_patch(&mut a, &json!({"pinned": null, "extra": null})).unwrap();
        assert!(!a.pinned);
        assert!(a.extra.is_empty());
    }

    #[test]
    fn friend_allowed_patchable() {
        let mut a = Account::default();
        apply_patch(&mut a, &json!({"friendAllowed": true})).unwrap();
        assert!(a.friend_allowed);
        // revoking access ends a pause the friend can no longer lift
        a.friend_paused = true;
        apply_patch(&mut a, &json!({"note": "x"})).unwrap();
        assert!(a.friend_paused, "other edits keep it");
        apply_patch(&mut a, &json!({"friendAllowed": false})).unwrap();
        assert!(!a.friend_paused);
    }

    #[test]
    fn friend_paused_not_patchable() {
        let mut a = Account::default();
        assert!(apply_patch(&mut a, &json!({"friendPaused": true})).is_err());
        assert!(apply_patch(&mut a, &json!({"friendLastPlace": 1})).is_err());
        assert!(apply_patch(&mut a, &json!({"friendAdded": true})).is_err());
        assert!(!a.friend_paused);
    }

    #[test]
    fn friend_fields_roundtrip() {
        let a = Account {
            friend_allowed: true,
            friend_paused: true,
            friend_last_place: Some(7),
            ..Default::default()
        };
        let b: Account = serde_json::from_value(serde_json::to_value(&a).unwrap()).unwrap();
        assert_eq!(
            (b.friend_allowed, b.friend_paused, b.friend_last_place),
            (true, true, Some(7))
        );
    }

    #[test]
    fn target_for_skips_unset_own_target() {
        let mut p = VaultPayload::default();
        let (_, id) = upsert(&mut p, &who(9, "g"), None, None);
        update(
            &mut p,
            &id,
            &json!({"group": "G", "launchTarget": {"jobId": "j"}}),
        )
        .unwrap();
        set_group_target(
            &mut p,
            "G",
            Some(LaunchTarget {
                place_id: Some(9),
                ..Default::default()
            }),
        )
        .unwrap();
        assert_eq!(target_for(&p, &p.accounts[0]).unwrap().place_id, Some(9));
    }
}

//! Discover commands (docs/contracts/discover.md): server streams, find a player, regions,
//! "set server, then join", games, universe, favorites.

pub mod favorites;

use crate::accounts::cmds::{emit_changed, expire_session};
use crate::commands::{guard, CmdResult, Gen, LOCKED, WAS_LOCKED};
use crate::launch::{
    dest::{parse_destination, parse_server, Destination, ServerTarget},
    launcher::{launch_start, library, BatchId},
    resolve::{cached, store, Bar, Cache, Mode, TTL},
};
use crate::roblox::{
    api::{gamejoin, games, servers, servers::Kind, thumbnails, users},
    RobloxClient, RobloxError, Secret,
};
use crate::AppState;
use favorites::Favorite;
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    net::IpAddr,
    path::Path,
    sync::{Arc, LazyLock, Mutex, PoisonError},
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};
use tokio_util::sync::CancellationToken;

const PAGE: &str = "servers://page";
const FIND: &str = "servers://find";
/// 100 pages = 10k rows, the table's cap.
const MAX_PAGES: u32 = 100;
const HEAD: &str = "150x150";

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub job_id: String,
    pub playing: u32,
    pub max_players: u32,
    pub ping: Option<u32>,
    pub fps: Option<f64>,
    pub kind: Kind,
    pub name: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub stream_id: String,
    pub rows: Vec<Row>,
    pub page: u32,
    pub done: bool,
    pub error: Option<String>,
    /// Ended by a cancel (stop, newer stream, lock), not by running out of pages.
    pub stopped: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Found {
    pub job_id: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FindProgress {
    pub stream_id: String,
    pub pages_scanned: u32,
    pub found: Option<Found>,
    pub done: bool,
    pub error: Option<String>,
    /// Ended by a cancel (stop, newer stream, lock), not by running out of pages.
    pub stopped: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Region {
    pub label: String,
    pub country_code: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Tile {
    pub universe_id: u64,
    pub place_id: u64,
    pub name: String,
    pub creator: Option<String>,
    pub playing: Option<u64>,
    pub like_ratio: Option<f64>,
    pub thumbnail: Option<String>,
    pub icon: Option<String>,
}

// ---- pure logic ----

/// Lookup URL for `ip`; None unless the template is http(s) and has `{ip}`.
pub fn geo_url(template: &str, ip: IpAddr) -> Option<String> {
    if !template.contains("{ip}") {
        return None;
    }
    let u = template.replace("{ip}", &ip.to_string());
    let ok = url::Url::parse(&u).is_ok_and(|x| matches!(x.scheme(), "http" | "https"));
    ok.then_some(u)
}

/// `{key}` placeholders filled from the lookup JSON. Missing keys drop out with their
/// separator; None when nothing is left or the lookup says `status: fail`.
pub fn format_region(format: &str, v: &Value) -> Option<Region> {
    static KEY: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\{(\w+)\}").expect("valid regex"));
    // two or more separators in a row (a key dropped out between them) -> the first one
    static SEPS: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"\s*([,|/·-])(?:\s*[,|/·-])+\s*").expect("valid regex")
    });
    if v.get("status").and_then(Value::as_str) == Some("fail") {
        return None;
    }
    let filled = KEY.replace_all(format, |c: &regex::Captures| match v.get(&c[1]) {
        Some(Value::String(s)) => s.clone(),
        Some(n @ (Value::Number(_) | Value::Bool(_))) => n.to_string(),
        _ => String::new(),
    });
    let label = SEPS.replace_all(&filled, "$1 ");
    let label = label.trim_matches(|c: char| c.is_whitespace() || ",|/·-".contains(c));
    (!label.is_empty()).then(|| Region {
        label: label.to_string(),
        country_code: v
            .get("countryCode")
            .and_then(Value::as_str)
            .filter(|c| !c.is_empty())
            .map(str::to_owned),
    })
}

pub fn like_ratio(up: u64, down: u64) -> Option<f64> {
    let total = up + down;
    (total > 0).then(|| up as f64 / total as f64)
}

/// First server (in page order) with a player whose headshot equals `target`.
pub fn found_job(
    pairs: &[(String, String)],
    urls: &HashMap<String, String>,
    target: &str,
) -> Option<String> {
    pairs
        .iter()
        .find(|(_, t)| urls.get(t).is_some_and(|u| u == target))
        .map(|(j, _)| j.clone())
}

fn to_row(s: servers::Server, kind: Kind) -> Option<Row> {
    Some(Row {
        job_id: s.id?,
        playing: s.playing,
        max_players: s.max_players,
        ping: s.ping.map(|p| p.round() as u32),
        fps: s.fps.map(|f| (f * 10.0).round() / 10.0),
        kind,
        name: s.name,
    })
}

fn err_text(e: RobloxError) -> String {
    e.to_string()
}

// ---- streams ----

/// stream id → (window label, event, token).
type Streams = HashMap<String, (String, &'static str, CancellationToken)>;
static STREAMS: LazyLock<Mutex<Streams>> = LazyLock::new(Default::default);

/// New stream for `window`; cancels that window's previous stream of the same event.
pub fn open_stream(window: &str, event: &'static str) -> (String, CancellationToken) {
    let mut g = STREAMS.lock().unwrap_or_else(PoisonError::into_inner);
    g.retain(|_, (w, e, t)| {
        let same = w == window && *e == event;
        if same {
            t.cancel();
        }
        !same
    });
    let id = crate::accounts::new_id();
    let t = CancellationToken::new();
    g.insert(id.clone(), (window.to_string(), event, t.clone()));
    (id, t)
}

/// Cancels and drops every stream whose window label matches (vault lock: all; closed
/// window: its label).
pub fn cancel_streams(window: impl Fn(&str) -> bool) {
    STREAMS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|_, (w, _, t)| {
            let hit = window(w);
            if hit {
                t.cancel();
            }
            !hit
        });
}

pub fn close_stream(id: &str) {
    let gone = STREAMS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(id);
    if let Some((_, _, t)) = gone {
        t.cancel();
    }
}

/// Pages 1.. until the last page, an error (one page with `error`, then stop), cancel
/// (one empty `stopped` page) or MAX_PAGES. `true` = it ended on a 401 (cookie rejected).
pub async fn run_pages(
    c: &RobloxClient,
    place_id: u64,
    kind: Kind,
    cookie: Option<&Secret>,
    cancel: &CancellationToken,
    stream_id: &str,
    emit: &(dyn Fn(Page) + Send + Sync),
) -> bool {
    let mut cursor: Option<String> = None;
    let stop = |n: u32| {
        emit(Page {
            stream_id: stream_id.into(),
            rows: vec![],
            page: n - 1,
            done: true,
            error: None,
            stopped: true,
        });
        false
    };
    for n in 1..=MAX_PAGES {
        if cancel.is_cancelled() {
            return stop(n);
        }
        let fetch = servers::page(c, place_id, kind, cursor.as_deref(), cookie);
        let Some(r) = cancel.run_until_cancelled(fetch).await else {
            return stop(n);
        };
        let page = |rows, done, error| Page {
            stream_id: stream_id.into(),
            rows,
            page: n,
            done,
            error,
            stopped: false,
        };
        match r {
            Err(e) => {
                let dead = matches!(e, RobloxError::Unauthorized);
                emit(page(vec![], true, Some(err_text(e))));
                return dead;
            }
            Ok((list, next)) => {
                let done = next.is_none() || n == MAX_PAGES;
                let rows = list.into_iter().filter_map(|s| to_row(s, kind)).collect();
                emit(page(rows, done, None));
                if done {
                    return false;
                }
                cursor = next;
            }
        }
    }
    false
}

/// The terminal event of a cancelled scan.
fn find_stopped(stream_id: &str, pages_scanned: u32) -> FindProgress {
    FindProgress {
        stream_id: stream_id.into(),
        pages_scanned,
        found: None,
        done: true,
        error: None,
        stopped: true,
    }
}

/// Scans authenticated pages, matching each server's player tokens (as headshot URLs)
/// against `target`; progress after every page. `true` = it ended on a 401 (cookie rejected).
pub async fn run_find(
    c: &RobloxClient,
    place_id: u64,
    target: &str,
    cookie: &Secret,
    cancel: &CancellationToken,
    stream_id: &str,
    emit: &(dyn Fn(FindProgress) + Send + Sync),
) -> bool {
    let mut cursor: Option<String> = None;
    for n in 1..=MAX_PAGES {
        if cancel.is_cancelled() {
            emit(find_stopped(stream_id, n - 1));
            return false;
        }
        let cur = cursor.clone();
        let step = async move {
            let (list, next) =
                servers::page(c, place_id, Kind::Public, cur.as_deref(), Some(cookie)).await?;
            let pairs: Vec<(String, String)> = list
                .into_iter()
                .filter_map(|s| Some((s.id?, s.player_tokens)))
                .flat_map(|(id, ts)| ts.into_iter().map(move |t| (id.clone(), t)))
                .collect();
            let mut tokens: Vec<String> = pairs.iter().map(|p| p.1.clone()).collect();
            tokens.sort_unstable();
            tokens.dedup();
            let urls = thumbnails::headshots_by_token(c, &tokens, HEAD).await?;
            Ok::<_, RobloxError>((found_job(&pairs, &urls, target), next))
        };
        let Some(r) = cancel.run_until_cancelled(step).await else {
            emit(find_stopped(stream_id, n - 1));
            return false;
        };
        let progress = |pages_scanned, found, done, error| FindProgress {
            stream_id: stream_id.into(),
            pages_scanned,
            found,
            done,
            error,
            stopped: false,
        };
        match r {
            Err(e) => {
                let dead = matches!(e, RobloxError::Unauthorized);
                emit(progress(n - 1, None, true, Some(err_text(e))));
                return dead;
            }
            Ok((found, next)) => {
                let done = found.is_some() || next.is_none() || n == MAX_PAGES;
                emit(progress(
                    n,
                    found.map(|job_id| Found { job_id }),
                    done,
                    None,
                ));
                if done {
                    return false;
                }
                cursor = next;
            }
        }
    }
    false
}

// ---- helpers ----

/// Roblox rejected `account_id`'s cookie (401): mark the session expired, refresh the list.
fn session_dead(app: &AppHandle, account_id: &str, cookie: &Secret) {
    match expire_session(&app.state::<AppState>(), account_id, cookie) {
        Ok(true) => emit_changed(app),
        Ok(false) => {}
        Err(e) => log::debug!("session state not saved: {e}"),
    }
}

fn cookie_of(st: &AppState, account_id: &str) -> CmdResult<Secret> {
    let g = guard(&st.vault);
    let p = g.as_ref().ok_or(LOCKED)?.payload();
    p.secrets
        .get(account_id)
        .filter(|s| !s.cookie.is_empty())
        .map(|s| Secret::new(s.cookie.as_str()))
        .ok_or_else(|| "Sign in to this account first.".into())
}

/// `open_stream` for a stream holding a cookie copied after `gen`: a lock in between ran
/// `cancel_streams` before this stream existed, so close it here instead.
fn cookie_stream(
    st: &AppState,
    gen: Option<Gen>,
    label: &str,
    event: &'static str,
) -> CmdResult<(String, CancellationToken)> {
    let (id, token) = open_stream(label, event);
    if gen.is_some_and(|g| guard(&st.vault).is_none() || !g.current(&st.lock_gen)) {
        close_stream(&id);
        return Err(WAS_LOCKED.into());
    }
    Ok((id, token))
}

fn setting(st: &AppState, p: &str) -> Value {
    st.settings
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .pointer(p)
        .cloned()
        .unwrap_or(Value::Null)
}

fn emit_to<T: Serialize + Clone>(app: &AppHandle, label: &str, event: &str, p: T) {
    if let Err(e) = app.emit_to(label, event, p) {
        log::warn!("could not emit {event}: {e}");
    }
}

// ---- server commands ----

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamId {
    stream_id: String,
}

#[tauri::command]
pub async fn servers_stream(
    app: AppHandle,
    window: WebviewWindow,
    place_id: u64,
    kind: Kind,
    account_id: Option<String>,
) -> CmdResult<StreamId> {
    let st = app.state::<AppState>();
    let gen = Gen::now(&st.lock_gen);
    // Public reads never send a cookie.
    let acct = match (kind, account_id) {
        (Kind::Public, _) => None,
        (Kind::Private, Some(id)) => Some((cookie_of(&st, &id)?, id)),
        (Kind::Private, None) => return Err("Private servers need a selected account.".into()),
    };
    let label = window.label().to_string();
    let (id, token) = cookie_stream(&st, acct.is_some().then_some(gen), &label, PAGE)?;
    let (c, sid) = (st.roblox.clone(), id.clone());
    tauri::async_runtime::spawn(async move {
        let emit = |p: Page| emit_to(&app, &label, PAGE, p);
        let cookie = acct.as_ref().map(|a| &a.0);
        if run_pages(&c, place_id, kind, cookie, &token, &sid, &emit).await {
            if let Some((ck, acc)) = &acct {
                session_dead(&app, acc, ck);
            }
        }
        close_stream(&sid);
    });
    Ok(StreamId { stream_id: id })
}

#[tauri::command]
pub async fn servers_stop(stream_id: String) -> CmdResult<()> {
    close_stream(&stream_id);
    Ok(())
}

fn valid_username(u: &str) -> bool {
    (3..=20).contains(&u.len()) && u.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Username → headshot URL (same size as the token batch).
async fn target_headshot(c: &RobloxClient, username: &str) -> Result<String, String> {
    let u = users::by_usernames(c, &[username.to_string()])
        .await
        .map_err(err_text)?
        .into_iter()
        .next()
        .ok_or("No Roblox user has that name.")?;
    // Pending right after an avatar change: one retry.
    for retry in [true, false] {
        let url = thumbnails::headshots(c, &[u.id], HEAD)
            .await
            .map_err(err_text)?
            .into_iter()
            .find(|t| t.state == "Completed")
            .and_then(|t| t.image_url);
        match url {
            Some(u) => return Ok(u),
            None if retry => tokio::time::sleep(Duration::from_secs(2)).await,
            None => {}
        }
    }
    Err("Couldn't get their avatar. Try again later.".into())
}

#[tauri::command]
pub async fn servers_find_player(
    app: AppHandle,
    window: WebviewWindow,
    place_id: u64,
    username: String,
    account_id: String,
) -> CmdResult<StreamId> {
    let username = username.trim().to_string();
    if !valid_username(&username) {
        return Err("No Roblox user has that name.".into());
    }
    let st = app.state::<AppState>();
    let gen = Gen::now(&st.lock_gen);
    let cookie = cookie_of(&st, &account_id)?;
    let label = window.label().to_string();
    let (id, token) = cookie_stream(&st, Some(gen), &label, FIND)?;
    let (c, sid) = (st.roblox.clone(), id.clone());
    tauri::async_runtime::spawn(async move {
        let emit = |p: FindProgress| emit_to(&app, &label, FIND, p);
        match token
            .run_until_cancelled(target_headshot(&c, &username))
            .await
        {
            None => emit(find_stopped(&sid, 0)),
            Some(Err(e)) => emit(FindProgress {
                error: Some(e),
                stopped: false,
                ..find_stopped(&sid, 0)
            }),
            Some(Ok(t)) if run_find(&c, place_id, &t, &cookie, &token, &sid, &emit).await => {
                session_dead(&app, &account_id, &cookie);
            }
            Some(Ok(_)) => {}
        }
        close_stream(&sid);
    });
    Ok(StreamId { stream_id: id })
}

static JOB_REGIONS: Cache<Option<Region>> = LazyLock::new(Default::default);
static IP_REGIONS: Cache<Region> = LazyLock::new(Default::default);
/// ip-api's free tier allows 45/min.
const GEO_PER_MIN: u32 = 40;
/// ponytail: per-call cap; gamejoin's per-account 20/min makes bigger asks slow anyway.
const MAX_REGION_JOBS: usize = 50;

/// Cached per job: a hit for 30 min, a miss (no IP, `status: fail`, bad template) for 2 min
/// so reloads don't spend the gamejoin budget again. Errors aren't cached.
async fn region_of(
    c: &RobloxClient,
    place_id: u64,
    job: &str,
    cookie: &Secret,
    tpl: &str,
    fmt: &str,
) -> Result<Option<Region>, RobloxError> {
    // The value depends on provider and format too: a settings change must miss.
    let key = format!("{tpl}\n{fmt}\n{job}");
    if let Some(r) = cached(&JOB_REGIONS, &key) {
        return Ok(r);
    }
    let r = lookup_region(c, place_id, job, cookie, tpl, fmt).await?;
    let ttl = if r.is_some() { 1_800 } else { 120 };
    store(&JOB_REGIONS, key, r.clone(), Duration::from_secs(ttl));
    Ok(r)
}

async fn lookup_region(
    c: &RobloxClient,
    place_id: u64,
    job: &str,
    cookie: &Secret,
    tpl: &str,
    fmt: &str,
) -> Result<Option<Region>, RobloxError> {
    let Some(ip) = gamejoin::server_ip(&gamejoin::join_instance(c, place_id, job, cookie).await?)
    else {
        return Ok(None);
    };
    let key = format!("{tpl}\n{fmt}\n{ip}");
    if let Some(r) = cached(&IP_REGIONS, &key) {
        return Ok(Some(r));
    }
    let Some(url) = geo_url(tpl, ip) else {
        return Ok(None);
    };
    // No cookie ever goes to the lookup host.
    let v = c.get_external_json(&url, "geo", GEO_PER_MIN).await?;
    let r = format_region(fmt, &v);
    if let Some(r) = &r {
        store(&IP_REGIONS, key, r.clone(), Duration::from_secs(86_400));
    }
    Ok(r)
}

/// Region per job. Invalid ids and real misses map to None so the UI doesn't ask again.
/// Unauthorized is an error; RateLimited, a lock (`live` false) or other errors leave ids
/// out (asked again later).
async fn regions_for(
    c: &RobloxClient,
    place_id: u64,
    job_ids: Vec<String>,
    cookie: &Secret,
    tpl: &str,
    fmt: &str,
    live: impl Fn() -> bool,
) -> CmdResult<HashMap<String, Option<Region>>> {
    let mut out = HashMap::new();
    let mut asked = 0;
    for job in job_ids {
        if out.contains_key(&job) {
            continue;
        }
        if !matches!(parse_server(&job), Some(ServerTarget::JobId(_))) {
            out.insert(job, None);
            continue;
        }
        // Locked meanwhile: no more calls with the copied cookie.
        if asked == MAX_REGION_JOBS || !live() {
            break;
        }
        asked += 1;
        let r = match region_of(c, place_id, &job, cookie, tpl, fmt).await {
            Ok(r) => r,
            Err(RobloxError::Unauthorized) => return Err(RobloxError::Unauthorized.to_string()),
            Err(RobloxError::RateLimited) => break,
            Err(e) => {
                log::debug!("region of {job}: {e}");
                continue; // transient: not cached
            }
        };
        out.insert(job, r);
    }
    Ok(out)
}

#[tauri::command]
pub async fn servers_regions(
    app: AppHandle,
    state: State<'_, AppState>,
    place_id: u64,
    job_ids: Vec<String>,
    account_id: String,
) -> CmdResult<HashMap<String, Option<Region>>> {
    if setting(&state, "/geo/enabled") == Value::Bool(false) {
        return Ok(HashMap::new());
    }
    let tpl = setting(&state, "/geo/urlTemplate");
    let fmt = setting(&state, "/geo/format");
    let tpl = tpl.as_str().unwrap_or("http://ip-api.com/json/{ip}");
    let fmt = fmt.as_str().unwrap_or("{city}, {countryCode}");
    let gen = Gen::now(&state.lock_gen);
    let cookie = cookie_of(&state, &account_id)?;
    let live = || gen.current(&state.lock_gen) && guard(&state.vault).is_some();
    let r = regions_for(&state.roblox, place_id, job_ids, &cookie, tpl, fmt, live).await;
    if r.is_err() {
        session_dead(&app, &account_id, &cookie); // regions_for fails only on a 401
    }
    r
}

pub const RESERVE_BUSY: &str = "RESERVE_BUSY";

/// Every answer was gamejoin's "game ended" (5) or "game full" (6): no account can join,
/// so the batch would only fail. Mixed or retry-later answers (0, 1) still launch.
fn closed_server(statuses: &[Option<i64>]) -> bool {
    !statuses.is_empty() && statuses.iter().all(|s| matches!(s, Some(5 | 6)))
}

#[tauri::command]
pub async fn servers_reserve_join(
    app: AppHandle,
    place_id: u64,
    job_id: String,
    mut account_ids: Vec<String>,
) -> CmdResult<BatchId> {
    if !matches!(parse_server(&job_id), Some(ServerTarget::JobId(_))) {
        return Err("Enter a server JobId.".into());
    }
    let mut seen = HashSet::new();
    account_ids.retain(|i| seen.insert(i.clone()));
    // One reserve loop at a time (double click, second window): the second never queues
    // a duplicate batch.
    static RESERVING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    struct Turn;
    impl Drop for Turn {
        fn drop(&mut self) {
            RESERVING.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }
    if RESERVING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return Err(RESERVE_BUSY.into());
    }
    let _turn = Turn;
    let st = app.state::<AppState>();
    // ponytail: all reservations first, then the staggered batch; per-launch reserve if
    // Roblox drops them before late accounts start.
    let mut statuses = vec![];
    for id in &account_ids {
        let Ok(ck) = cookie_of(&st, id) else { continue }; // the launcher reports it
        match gamejoin::join_instance(&st.roblox, place_id, &job_id, &ck).await {
            Ok(v) => {
                log::info!("reserve: gamejoin status {}", v["status"]);
                statuses.push(v["status"].as_i64());
            }
            Err(e) => log::warn!("reserve failed, launching anyway: {e}"),
        }
    }
    if closed_server(&statuses) {
        return Err("That server is full or has closed. Pick another one.".into());
    }
    let bar = Bar {
        destination: place_id.to_string(),
        server: job_id,
        mode: Mode::Specific,
        ..Default::default()
    };
    launch_start(app.clone(), account_ids, bar, true).await
}

// ---- games ----

static SESSION: LazyLock<String> = LazyLock::new(crate::accounts::new_id);
static TILES: Cache<Tile> = LazyLock::new(Default::default);
static LISTS: Cache<(Vec<Tile>, Option<String>)> = LazyLock::new(Default::default);

fn tile(h: games::Hit) -> Tile {
    Tile {
        universe_id: h.universe_id,
        place_id: h.root_place_id,
        name: h.name,
        creator: h.creator_name.filter(|c| !c.is_empty()),
        playing: h.player_count,
        like_ratio: like_ratio(
            h.total_up_votes.unwrap_or(0),
            h.total_down_votes.unwrap_or(0),
        ),
        thumbnail: None,
        icon: None,
    }
}

/// Thumbnails + icons; image failures leave them null.
async fn decorate(c: &RobloxClient, tiles: &mut [Tile]) {
    let mut ids: Vec<u64> = tiles.iter().map(|t| t.universe_id).collect();
    ids.sort_unstable();
    ids.dedup();
    let thumbs = thumbnails::game_thumbs(c, &ids).await.unwrap_or_else(|e| {
        log::debug!("game thumbnails: {e}");
        HashMap::new()
    });
    let icons = thumbnails::game_icons(c, &ids).await.unwrap_or_else(|e| {
        log::debug!("game icons: {e}");
        HashMap::new()
    });
    for t in tiles {
        t.thumbnail = thumbs.get(&t.universe_id).cloned();
        t.icon = icons.get(&t.universe_id).cloned();
    }
}

async fn list_cached(
    c: &RobloxClient,
    key: String,
    f: impl std::future::Future<Output = Result<(Vec<games::Hit>, Option<String>), RobloxError>>,
) -> CmdResult<(Vec<Tile>, Option<String>)> {
    if let Some(v) = cached(&LISTS, &key) {
        return Ok(v);
    }
    let (hits, next) = f.await.map_err(err_text)?;
    let mut tiles: Vec<Tile> = hits.into_iter().map(tile).collect();
    decorate(c, &mut tiles).await;
    let next = next.filter(|_| !tiles.is_empty());
    store(&LISTS, key, (tiles.clone(), next.clone()), TTL);
    Ok((tiles, next))
}

#[tauri::command]
pub async fn games_discover(state: State<'_, AppState>) -> CmdResult<Vec<Tile>> {
    let c = &state.roblox;
    let f = async { Ok((games::top_sort(c, &SESSION).await?, None)) };
    Ok(list_cached(c, "discover".into(), f).await?.0)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchPage {
    games: Vec<Tile>,
    next_page_token: Option<String>,
}

#[tauri::command]
pub async fn games_search(
    state: State<'_, AppState>,
    query: String,
    page_token: Option<String>,
) -> CmdResult<SearchPage> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(SearchPage {
            games: vec![],
            next_page_token: None,
        });
    }
    if q.chars().count() > 100 {
        return Err("Search for 100 characters or fewer.".into());
    }
    if page_token.as_ref().is_some_and(|t| t.len() > 2000) {
        return Err("Couldn't load more results. Search again.".into());
    }
    let c = &state.roblox;
    let key = format!("s\n{q}\n{}", page_token.as_deref().unwrap_or(""));
    let (games, next_page_token) =
        list_cached(c, key, games::search(c, q, page_token.as_deref(), &SESSION)).await?;
    Ok(SearchPage {
        games,
        next_page_token,
    })
}

/// Tiles for places (input order; places that fail to resolve are skipped). 10 min cache.
async fn details(c: &RobloxClient, place_ids: &[u64]) -> CmdResult<Vec<Tile>> {
    let mut missing: Vec<(u64, u64)> = vec![]; // (place, universe)
    for &p in place_ids {
        if cached(&TILES, &p.to_string()).is_some() || missing.iter().any(|m| m.0 == p) {
            continue;
        }
        match games::universe_of(c, p).await {
            Ok(u) => missing.push((p, u)),
            Err(RobloxError::RateLimited) => return Err(RobloxError::RateLimited.to_string()),
            Err(e) => log::debug!("universe of {p}: {e}"),
        }
    }
    if !missing.is_empty() {
        let unis: Vec<u64> = missing.iter().map(|m| m.1).collect();
        let info: HashMap<u64, games::Game> = games::by_universe(c, &unis)
            .await
            .map_err(err_text)?
            .into_iter()
            .map(|g| (g.id, g))
            .collect();
        let votes = games::votes(c, &unis).await.unwrap_or_else(|e| {
            log::debug!("votes: {e}");
            HashMap::new()
        });
        let mut tiles: Vec<(u64, Tile)> = missing
            .iter()
            .filter_map(|&(p, u)| {
                let g = info.get(&u)?;
                let (up, down) = votes.get(&u).copied().unwrap_or((0, 0));
                Some((
                    p,
                    Tile {
                        universe_id: u,
                        place_id: g.root_place_id.unwrap_or(p),
                        name: g.name.clone(),
                        creator: g.creator.as_ref().map(|c| c.name.clone()),
                        playing: g.playing,
                        like_ratio: like_ratio(up, down),
                        thumbnail: None,
                        icon: None,
                    },
                ))
            })
            .collect();
        let mut only: Vec<Tile> = tiles.iter().map(|t| t.1.clone()).collect();
        decorate(c, &mut only).await;
        for ((p, t), d) in tiles.iter_mut().zip(only) {
            *t = d;
            store(&TILES, p.to_string(), t.clone(), TTL);
        }
    }
    Ok(place_ids
        .iter()
        .filter_map(|p| cached(&TILES, &p.to_string()))
        .collect())
}

#[tauri::command]
pub async fn game_details(
    state: State<'_, AppState>,
    mut place_ids: Vec<u64>,
) -> CmdResult<Vec<Tile>> {
    let mut seen = HashSet::new();
    place_ids.retain(|p| seen.insert(*p));
    place_ids.truncate(100);
    details(&state.roblox, &place_ids).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UniverseId {
    universe_id: u64,
}

#[tauri::command]
pub async fn universe_of(state: State<'_, AppState>, place_id: u64) -> CmdResult<UniverseId> {
    Ok(UniverseId {
        universe_id: games::universe_of(&state.roblox, place_id)
            .await
            .map_err(err_text)?,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UniversePlace {
    place_id: u64,
    name: String,
    is_root: bool,
}

#[tauri::command]
pub async fn universe_places(
    state: State<'_, AppState>,
    universe_id: u64,
) -> CmdResult<Vec<UniversePlace>> {
    let c = &state.roblox;
    let root = games::by_universe(c, &[universe_id])
        .await
        .map_err(err_text)?
        .into_iter()
        .next()
        .and_then(|g| g.root_place_id);
    Ok(games::places(c, universe_id)
        .await
        .map_err(err_text)?
        .into_iter()
        .map(|(place_id, name)| UniversePlace {
            place_id,
            name,
            is_root: Some(place_id) == root,
        })
        .collect())
}

// ---- favorites ----

async fn on_library<T: Send + 'static>(
    app: &AppHandle,
    f: impl FnOnce(&Path) -> std::io::Result<T> + Send + 'static,
) -> CmdResult<T> {
    // FIFO (tokio's Mutex is fair): edits land on disk in call order, and a list never
    // overtakes an edit sent before it. The file lock alone serialises but doesn't order.
    static TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _turn = TURN.lock().await;
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || f(&library(&app.state::<AppState>())))
        .await
        .map_err(|e| format!("internal error: {e}"))?
        .map_err(|e| format!("Could not save favorites: {e}"))
}

fn clean_name(n: &str) -> CmdResult<String> {
    let n = n.trim();
    if n.is_empty() || n.chars().count() > 100 {
        return Err("Use a name of 1 to 100 characters.".into());
    }
    Ok(n.to_string())
}

/// Empty = none; otherwise it must be a private server or share link.
fn clean_link(l: Option<&str>) -> CmdResult<Option<String>> {
    let Some(l) = l.map(str::trim).filter(|l| !l.is_empty()) else {
        return Ok(None);
    };
    match parse_destination(l) {
        Some(Destination::Private { .. } | Destination::Share { .. }) => Ok(Some(l.to_string())),
        _ => Err("Use a private server link.".into()),
    }
}

#[tauri::command]
pub async fn favorites_list(app: AppHandle) -> CmdResult<Vec<Favorite>> {
    on_library(&app, favorites::list).await
}

#[tauri::command]
pub async fn favorites_add(
    app: AppHandle,
    place_id: u64,
    name: String,
    private_link: Option<String>,
) -> CmdResult<Favorite> {
    let name = clean_name(&name)?;
    let private_link = clean_link(private_link.as_deref())?;
    let c: Arc<RobloxClient> = app.state::<AppState>().roblox.clone();
    let thumbnail = details(&c, &[place_id])
        .await
        .ok()
        .and_then(|t| t.into_iter().next())
        .and_then(|t| t.thumbnail.or(t.icon));
    let f = Favorite {
        id: crate::accounts::new_id(),
        place_id,
        name,
        private_link,
        thumbnail,
        order: 0,
    };
    on_library(&app, move |p| favorites::add(p, f)).await
}

#[tauri::command]
pub async fn favorites_update(app: AppHandle, id: String, patch: Value) -> CmdResult<Favorite> {
    let (name, link) = parse_patch(&patch)?;
    on_library(&app, move |p| favorites::update(p, &id, name, link))
        .await?
        .ok_or_else(|| "Favorite not found.".into())
}

/// `{name?, privateLink?: string|null}` -> (new name, new link; `Some(None)` clears it).
type Patch = (Option<String>, Option<Option<String>>);
fn parse_patch(patch: &Value) -> CmdResult<Patch> {
    let name = match patch.get("name") {
        Some(v) => Some(clean_name(
            v.as_str().ok_or("Use a name of 1 to 100 characters.")?,
        )?),
        None => None,
    };
    let link = match patch.get("privateLink") {
        Some(Value::Null) => Some(None),
        Some(Value::String(s)) => Some(clean_link(Some(s))?),
        Some(_) => return Err("Use a private server link.".into()),
        None => None,
    };
    Ok((name, link))
}

#[tauri::command]
pub async fn favorites_remove(app: AppHandle, id: String) -> CmdResult<()> {
    on_library(&app, move |p| favorites::remove(p, &id)).await
}

#[tauri::command]
pub async fn favorites_reorder(app: AppHandle, ids: Vec<String>) -> CmdResult<()> {
    on_library(&app, move |p| favorites::reorder(p, &ids)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        extract::Query,
        routing::{get, post},
        Json, Router,
    };
    use serde_json::json;
    use std::sync::atomic::{AtomicU32, Ordering::SeqCst};

    fn region(l: &str, cc: Option<&str>) -> Option<Region> {
        Some(Region {
            label: l.into(),
            country_code: cc.map(Into::into),
        })
    }

    #[test]
    fn geo_template_and_format() {
        let ip: IpAddr = "1.2.3.4".parse().unwrap();
        assert_eq!(
            geo_url("http://ip-api.com/json/{ip}", ip).as_deref(),
            Some("http://ip-api.com/json/1.2.3.4")
        );
        assert_eq!(
            geo_url("https://x.example/{ip}?f=1", ip).as_deref(),
            Some("https://x.example/1.2.3.4?f=1")
        );
        assert_eq!(geo_url("http://x.example/json", ip), None);
        assert_eq!(geo_url("file:///{ip}", ip), None);

        let v = json!({"status": "success", "city": "Frankfurt", "countryCode": "DE", "regionName": "Hesse", "lat": 50.1});
        let f = "{city}, {countryCode}";
        assert_eq!(format_region(f, &v), region("Frankfurt, DE", Some("DE")));
        assert_eq!(
            format_region("{city} ({regionName}) {lat}", &v),
            region("Frankfurt (Hesse) 50.1", Some("DE"))
        );
        // missing keys drop out with their separator
        assert_eq!(
            format_region(f, &json!({"countryCode": "NL"})),
            region("NL", Some("NL"))
        );
        assert_eq!(
            format_region("{city}, {nope}, {countryCode}", &v),
            region("Frankfurt, DE", Some("DE"))
        );
        assert_eq!(
            format_region(f, &json!({"city": "Tokyo", "countryCode": ""})),
            region("Tokyo", None)
        );
        assert_eq!(format_region(f, &json!({})), None);
        assert_eq!(
            format_region(f, &json!({"status": "fail", "message": "private range"})),
            None
        );
        assert_eq!(format_region("no keys", &v), region("no keys", Some("DE")));
    }

    #[test]
    fn like_ratio_handles_zero() {
        assert_eq!(like_ratio(0, 0), None);
        assert_eq!(like_ratio(3, 1), Some(0.75));
    }

    #[test]
    fn token_match_first_server() {
        let pairs = vec![
            ("j1".into(), "a".into()),
            ("j2".into(), "b".into()),
            ("j3".into(), "c".into()),
        ];
        let urls: HashMap<String, String> = [("a", "u/a"), ("b", "u/me"), ("c", "u/me")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(found_job(&pairs, &urls, "u/me").as_deref(), Some("j2"));
        assert_eq!(found_job(&pairs, &urls, "u/none"), None);
    }

    #[test]
    fn cookie_stream_closed_after_lock() {
        let d = tempfile::tempdir().unwrap();
        let st = crate::test_state(
            d.path(),
            RobloxClient::new(crate::roblox::BaseUrls::all("http://127.0.0.1:9")),
        );
        let gen = Gen::now(&st.lock_gen);
        let (_, t) = cookie_stream(&st, Some(gen), "cs", PAGE).unwrap();
        assert!(!t.is_cancelled());
        st.lock_gen.fetch_add(1, SeqCst); // locked after the cookie was copied
        assert_eq!(
            cookie_stream(&st, Some(gen), "cs2", PAGE).unwrap_err(),
            WAS_LOCKED
        );
        assert!(
            cookie_stream(&st, None, "cs3", PAGE).is_ok(),
            "public: no check"
        );
    }

    #[test]
    fn new_stream_cancels_same_window_and_event_only() {
        let (a, ta) = open_stream("w1", PAGE);
        let (_, tf) = open_stream("w1", FIND);
        let (_, tw) = open_stream("w2", PAGE);
        let (b, tb) = open_stream("w1", PAGE);
        assert_ne!(a, b);
        assert!(ta.is_cancelled());
        assert!(!tf.is_cancelled() && !tw.is_cancelled() && !tb.is_cancelled());
        close_stream(&b);
        assert!(tb.is_cancelled());
    }

    /// Public servers: 3 pages of 2 (`j{page}{n}`), page 2's second server holds token "me".
    async fn pages_server(hits: Arc<AtomicU32>, fail_on: u32) -> RobloxClient {
        let app = Router::new()
            .route(
                "/v1/games/5/servers/Public",
                get(move |Query(q): Query<HashMap<String, String>>| {
                    let hits = hits.clone();
                    async move {
                        let n: u32 = q.get("cursor").map_or(1, |c| c.parse().unwrap());
                        hits.fetch_add(1, SeqCst);
                        if n == fail_on {
                            return Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
                        }
                        let tok = |i: u32| if n == 2 && i == 2 { "me".to_string() } else { format!("t{n}{i}") };
                        Ok(Json(json!({
                            "nextPageCursor": if n < 3 { json!((n + 1).to_string()) } else { Value::Null },
                            "data": ([1, 2].map(|i| json!({"id": format!("j{n}{i}"), "playing": i, "maxPlayers": 4, "playerTokens": [tok(i)], "ping": 50.4, "fps": 59.96})))
                        })))
                    }
                }),
            )
            .route(
                "/v1/batch",
                post(|Json(b): Json<Vec<Value>>| async move {
                    let data: Vec<Value> = b.iter().map(|r| json!({"requestId": r["requestId"], "state": "Completed",
                        "imageUrl": format!("https://tr.rbxcdn.com/{}", r["token"].as_str().unwrap())})).collect();
                    Json(json!({ "data": data }))
                }),
            );
        crate::roblox::mock(app).await
    }

    fn collect<T: Send + 'static>() -> (Arc<Mutex<Vec<T>>>, impl Fn(T) + Send + Sync) {
        let v = Arc::new(Mutex::new(vec![]));
        let v2 = v.clone();
        (v, move |p| v2.lock().unwrap().push(p))
    }

    #[tokio::test]
    async fn streams_all_pages_then_done() {
        let c = pages_server(Arc::default(), 0).await;
        let (got, emit) = collect::<Page>();
        run_pages(
            &c,
            5,
            Kind::Public,
            None,
            &CancellationToken::new(),
            "s",
            &emit,
        )
        .await;
        let g = got.lock().unwrap().clone();
        assert_eq!(
            g.iter()
                .map(|p| (p.page, p.done, p.rows.len()))
                .collect::<Vec<_>>(),
            [(1, false, 2), (2, false, 2), (3, true, 2)]
        );
        assert_eq!(
            g[0].rows[0],
            Row {
                job_id: "j11".into(),
                playing: 1,
                max_players: 4,
                ping: Some(50),
                fps: Some(60.0),
                kind: Kind::Public,
                name: None
            }
        );
        assert_eq!(g[0].stream_id, "s");
    }

    #[tokio::test]
    async fn error_ends_stream_with_error_page() {
        let c = pages_server(Arc::default(), 2).await;
        let (got, emit) = collect::<Page>();
        run_pages(
            &c,
            5,
            Kind::Public,
            None,
            &CancellationToken::new(),
            "s",
            &emit,
        )
        .await;
        let g = got.lock().unwrap().clone();
        assert_eq!(g.len(), 2);
        assert!(g[1].done && g[1].rows.is_empty() && g[1].error.is_some());
    }

    #[tokio::test]
    async fn cancel_stops_further_pages() {
        let hits = Arc::new(AtomicU32::new(0));
        let c = pages_server(hits.clone(), 0).await;
        let tok = CancellationToken::new();
        let (got, emit) = collect::<Page>();
        let t2 = tok.clone();
        let emit2 = move |p: Page| {
            emit(p);
            t2.cancel(); // stop right after the first page
        };
        run_pages(&c, 5, Kind::Public, None, &tok, "s", &emit2).await;
        let g = got.lock().unwrap().clone();
        assert_eq!(hits.load(SeqCst), 1);
        // then one terminal page, so the UI never sits at "streaming"
        assert_eq!(g.len(), 2);
        assert!(!g[0].stopped);
        assert!(g[1].done && g[1].stopped && g[1].rows.is_empty() && g[1].error.is_none());
        assert_eq!(g[1].page, 1);
    }

    #[tokio::test]
    async fn find_matches_token_on_page_two() {
        let c = pages_server(Arc::default(), 0).await;
        let (got, emit) = collect::<FindProgress>();
        let ck = Secret::new("c");
        run_find(
            &c,
            5,
            "https://tr.rbxcdn.com/me",
            &ck,
            &CancellationToken::new(),
            "f",
            &emit,
        )
        .await;
        let g = got.lock().unwrap().clone();
        assert_eq!(g.len(), 2);
        assert_eq!(
            (g[0].pages_scanned, g[0].done, g[0].found.clone()),
            (1, false, None)
        );
        assert_eq!((g[1].pages_scanned, g[1].done), (2, true));
        assert_eq!(
            g[1].found,
            Some(Found {
                job_id: "j22".into()
            })
        );
        drop(g);

        let (got, emit) = collect::<FindProgress>();
        run_find(
            &c,
            5,
            "https://tr.rbxcdn.com/nobody",
            &ck,
            &CancellationToken::new(),
            "f",
            &emit,
        )
        .await;
        let g = got.lock().unwrap().clone();
        assert_eq!(
            g.last().map(|p| (p.pages_scanned, p.done, p.found.clone())),
            Some((3, true, None))
        );
    }

    #[test]
    fn closed_server_only_when_every_answer_says_so() {
        assert!(closed_server(&[Some(6), Some(5)]));
        assert!(!closed_server(&[]));
        assert!(!closed_server(&[Some(6), Some(2)]));
        assert!(!closed_server(&[Some(1)]));
        assert!(!closed_server(&[None]));
    }

    #[test]
    fn cancel_streams_by_window() {
        let (_, a) = open_stream("cw1", PAGE);
        let (_, b) = open_stream("cw1", FIND);
        let (_, other) = open_stream("cw2", PAGE);
        cancel_streams(|l| l == "cw1");
        assert!(a.is_cancelled() && b.is_cancelled());
        assert!(!other.is_cancelled());
        cancel_streams(|l| l == "cw2");
        assert!(other.is_cancelled());
    }

    #[tokio::test]
    async fn regions_cache_misses_mark_invalid_and_stop_on_401() {
        const A: &str = "11111111-1111-4111-8111-111111111111";
        const B: &str = "22222222-2222-4222-8222-222222222222";
        const C: &str = "33333333-3333-4333-8333-333333333333";
        const D: &str = "44444444-4444-4444-8444-444444444444";
        const E: &str = "55555555-5555-4555-8555-555555555555";
        let hits = Arc::new(AtomicU32::new(0));
        let h = hits.clone();
        let app = Router::new()
            .route(
                "/v1/join-game-instance",
                post(move |Json(b): Json<Value>| {
                    let h = h.clone();
                    async move {
                        h.fetch_add(1, SeqCst);
                        match b["gameId"].as_str().unwrap() {
                            A => Ok(Json(
                                json!({"status": 2, "joinScript": {"MachineAddress": "9.9.9.1"}}),
                            )),
                            C => Err(axum::http::StatusCode::UNAUTHORIZED),
                            D => Err(axum::http::StatusCode::BAD_GATEWAY),
                            _ => Ok(Json(json!({"status": 0}))),
                        }
                    }
                }),
            )
            .route(
                "/geo",
                get(|| async { Json(json!({"city": "Paris", "countryCode": "FR"})) }),
            );
        let u = crate::roblox::mock_url(app).await;
        let c = RobloxClient::new(crate::roblox::BaseUrls::all(&u));
        let tpl = format!("{u}/geo?ip={{ip}}");
        let fmt = "{city}, {countryCode}";
        let ck = Secret::new("c");
        let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();

        let r = regions_for(&c, 5, ids(&[A, "bad", A, B]), &ck, &tpl, fmt, || true)
            .await
            .unwrap();
        assert_eq!(r.len(), 3);
        assert_eq!(r[A], region("Paris, FR", Some("FR")));
        assert_eq!((&r["bad"], &r[B]), (&None, &None));
        assert_eq!(
            hits.load(SeqCst),
            2,
            "duplicate and invalid ids never asked"
        );

        regions_for(&c, 5, ids(&[A, B]), &ck, &tpl, fmt, || true)
            .await
            .unwrap();
        assert_eq!(hits.load(SeqCst), 2, "hit and miss both cached");

        assert_eq!(
            regions_for(&c, 5, ids(&[C]), &ck, &tpl, fmt, || true).await,
            Err(RobloxError::Unauthorized.to_string())
        );

        // transient error (gamejoin 5xx): left out, asked again later
        let r = regions_for(&c, 5, ids(&[D]), &ck, &tpl, fmt, || true)
            .await
            .unwrap();
        assert!(r.is_empty());
        // locked: nothing asked with the copied cookie
        let before = hits.load(SeqCst);
        let r = regions_for(&c, 5, ids(&["bad", E]), &ck, &tpl, fmt, || false)
            .await
            .unwrap();
        assert_eq!((r.len(), hits.load(SeqCst)), (1, before));
    }

    #[test]
    fn clean_link_accepts_private_and_share_only() {
        assert_eq!(clean_link(None), Ok(None));
        assert_eq!(clean_link(Some("  ")), Ok(None));
        let pv = "https://www.roblox.com/games/5/x?privateServerLinkCode=123";
        assert_eq!(clean_link(Some(&format!(" {pv} "))), Ok(Some(pv.into())));
        let sh = "https://www.roblox.com/share?code=abc&type=Server";
        assert_eq!(clean_link(Some(sh)), Ok(Some(sh.into())));
        assert!(clean_link(Some("https://www.roblox.com/games/5/x")).is_err());
        assert!(clean_link(Some("hello")).is_err());
    }

    #[test]
    fn favorite_patch_parsing() {
        assert_eq!(parse_patch(&json!({})), Ok((None, None)));
        assert_eq!(
            parse_patch(&json!({"name": " Hub ", "privateLink": null})),
            Ok((Some("Hub".into()), Some(None)))
        );
        let sh = "https://www.roblox.com/share?code=abc&type=Server";
        assert_eq!(
            parse_patch(&json!({ "privateLink": sh })),
            Ok((None, Some(Some(sh.into()))))
        );
        assert!(parse_patch(&json!({"name": ""})).is_err());
        assert!(parse_patch(&json!({"name": 5})).is_err());
        assert!(parse_patch(&json!({"privateLink": 5})).is_err());
        assert!(parse_patch(&json!({"privateLink": "nope"})).is_err());
    }

    #[test]
    fn to_row_rounds_and_drops_idless() {
        let s = |id: Option<&str>| servers::Server {
            id: id.map(Into::into),
            playing: 1,
            max_players: 4,
            ping: Some(50.4),
            fps: Some(59.94),
            ..Default::default()
        };
        assert_eq!(to_row(s(None), Kind::Private), None);
        let r = to_row(s(Some("j")), Kind::Public).unwrap();
        assert_eq!(
            (r.job_id.as_str(), r.ping, r.fps),
            ("j", Some(50), Some(59.9))
        );
    }

    #[test]
    fn username_length_and_charset() {
        assert!(valid_username("abc"));
        assert!(valid_username(&"a".repeat(20)));
        assert!(valid_username("Ab_1"));
        assert!(!valid_username("ab"));
        assert!(!valid_username(&"a".repeat(21)));
        assert!(!valid_username("ab-cd"));
        assert!(!valid_username("ab cd"));
        assert!(!valid_username("abé"));
    }

    #[test]
    fn clean_name_trims_and_counts_chars() {
        assert_eq!(clean_name("  Hub  "), Ok("Hub".into()));
        assert!(clean_name("   ").is_err());
        assert!(clean_name(&"a".repeat(100)).is_ok());
        assert!(clean_name(&"a".repeat(101)).is_err());
        assert!(clean_name(&"é".repeat(100)).is_ok(), "100 chars, 200 bytes");
    }

    #[test]
    fn tile_drops_empty_creator_and_missing_votes() {
        let t = tile(games::Hit {
            universe_id: 1,
            root_place_id: 10,
            name: "G".into(),
            creator_name: Some(String::new()),
            ..Default::default()
        });
        assert_eq!((t.place_id, t.creator, t.like_ratio), (10, None, None));
    }

    #[tokio::test]
    async fn run_pages_reports_dead_cookie_and_plain_errors() {
        let app = Router::new().route(
            "/v1/games/5/servers/VIP",
            get(|| async { axum::http::StatusCode::UNAUTHORIZED }),
        );
        let c = crate::roblox::mock(app).await;
        let ck = Secret::new("c");
        let (got, emit) = collect::<Page>();
        let dead = run_pages(
            &c,
            5,
            Kind::Private,
            Some(&ck),
            &CancellationToken::new(),
            "s",
            &emit,
        )
        .await;
        assert!(dead);
        let g = got.lock().unwrap().clone();
        assert_eq!(g.len(), 1);
        assert_eq!(
            (g[0].done, g[0].error.as_deref()),
            (true, Some("Session expired, sign in again"))
        );
        drop(g);

        let (got, emit) = collect::<Page>();
        let dead = run_pages(
            &pages_server(Arc::default(), 1).await,
            5,
            Kind::Public,
            None,
            &CancellationToken::new(),
            "s",
            &emit,
        )
        .await;
        assert!(!dead, "a 500 is not a dead cookie");
        assert!(got.lock().unwrap()[0].error.is_some());
    }

    #[tokio::test]
    async fn run_find_dead_cookie_reports_dead() {
        let app = Router::new().route(
            "/v1/games/5/servers/Public",
            get(|| async { axum::http::StatusCode::UNAUTHORIZED }),
        );
        let c = crate::roblox::mock(app).await;
        let (got, emit) = collect::<FindProgress>();
        let ck = Secret::new("c");
        let dead = run_find(
            &c,
            5,
            "https://tr.rbxcdn.com/me",
            &ck,
            &CancellationToken::new(),
            "f",
            &emit,
        )
        .await;
        assert!(dead);
        let g = got.lock().unwrap().clone();
        assert_eq!(g.len(), 1);
        assert!(g[0].done && g[0].found.is_none() && g[0].error.is_some());
    }

    #[tokio::test]
    async fn target_headshot_unknown_user_errors() {
        let app = Router::new().route(
            "/v1/usernames/users",
            post(|| async { Json(json!({"data": []})) }),
        );
        let c = crate::roblox::mock(app).await;
        assert_eq!(
            target_headshot(&c, "nobody_here").await,
            Err("No Roblox user has that name.".to_string())
        );
    }

    #[tokio::test]
    async fn target_headshot_retries_once_while_pending() {
        let hits = Arc::new(AtomicU32::new(0));
        let h = hits.clone();
        let app = Router::new()
            .route(
                "/v1/usernames/users",
                post(|| async {
                    Json(json!({"data": [{"id": 7, "name": "bob", "displayName": "Bob", "requestedUsername": "bob"}]}))
                }),
            )
            .route(
                "/v1/users/avatar-headshot",
                get(move || {
                    let h = h.clone();
                    async move {
                        let pending = h.fetch_add(1, SeqCst) == 0;
                        let (state, url) = if pending {
                            ("Pending", Value::Null)
                        } else {
                            ("Completed", json!("https://tr.rbxcdn.com/bob"))
                        };
                        Json(json!({"data": [{"targetId": 7, "state": state, "imageUrl": url}]}))
                    }
                }),
            );
        let c = crate::roblox::mock(app).await;
        assert_eq!(
            target_headshot(&c, "bob").await,
            Ok("https://tr.rbxcdn.com/bob".to_string())
        );
        assert_eq!(hits.load(SeqCst), 2);
    }
}

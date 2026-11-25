//! Play-bar destination → preview chip (`destination_resolve`) and → `JoinRequest` (launcher).

use super::dest::{parse_destination, parse_server, Destination, ServerTarget, ShareKind};
use super::recents::Recent;
use super::uri::{JoinRequest, PrivateCode};
use crate::accounts::Presence;
use crate::roblox::{
    api::{games, presence, servers, thumbnails, users},
    RobloxClient, RobloxError, Secret,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex, PoisonError},
    time::{Duration, Instant},
};

pub(crate) const TTL: Duration = Duration::from_secs(600);
pub const SHARE_FAILED: &str = "Couldn't resolve this share link";

#[derive(Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Any,
    Specific,
    Random,
    Smallest,
}

#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Bar {
    pub destination: String,
    pub server: String,
    pub mode: Mode,
    /// Explicit opt-in (the Play bar): an empty bar opens Roblox on its home screen.
    /// Automation and auto-reconnect never set it, so an empty bar there still fails.
    pub home: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Follow {
    pub user_id: u64,
    pub username: String,
    pub presence: Option<Presence>,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub place_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub universe_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub playing: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub private_link_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub follow: Option<Follow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn err(kind: &'static str, e: impl Into<String>) -> Preview {
    Preview {
        kind,
        error: Some(e.into()),
        ..Default::default()
    }
}

#[derive(Clone)]
pub struct PlaceInfo {
    pub universe_id: u64,
    pub name: String,
    pub icon: Option<String>,
    pub playing: Option<u64>,
}

/// Key → (value, expiry). Expired entries are dropped on the next store.
pub(crate) type Cache<T> = LazyLock<Mutex<HashMap<String, (T, Instant)>>>;
static PLACES: Cache<PlaceInfo> = LazyLock::new(Default::default);
static PREVIEWS: Cache<Preview> = LazyLock::new(Default::default);

pub(crate) fn cached<T: Clone>(c: &Cache<T>, k: &str) -> Option<T> {
    let g = c.lock().unwrap_or_else(PoisonError::into_inner);
    g.get(k)
        .filter(|(_, exp)| *exp > Instant::now())
        .map(|(v, _)| v.clone())
}

pub(crate) fn store<T>(c: &Cache<T>, k: String, v: T, ttl: Duration) {
    let mut g = c.lock().unwrap_or_else(PoisonError::into_inner);
    let now = Instant::now();
    g.retain(|_, (_, exp)| *exp > now);
    g.insert(k, (v, now + ttl));
}

/// Name, icon and player count of a place (public calls, 10 min cache).
/// Cache only, no network.
pub fn cached_place(place_id: u64) -> Option<PlaceInfo> {
    cached(&PLACES, &place_id.to_string())
}

pub async fn place_info(c: &RobloxClient, place_id: u64) -> Result<PlaceInfo, RobloxError> {
    let k = place_id.to_string();
    if let Some(p) = cached(&PLACES, &k) {
        return Ok(p);
    }
    let universe_id = games::universe_of(c, place_id).await?;
    let g = games::by_universe(c, &[universe_id])
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| RobloxError::Decode("no game details".into()))?;
    let icon = match thumbnails::game_icon(c, universe_id).await {
        Ok(t) => t
            .filter(|t| t.state == "Completed")
            .and_then(|t| t.image_url),
        Err(e) => {
            log::debug!("game icon {universe_id}: {e}");
            None
        }
    };
    let p = PlaceInfo {
        universe_id,
        name: g.name,
        icon,
        playing: g.playing,
    };
    store(&PLACES, k, p.clone(), TTL);
    Ok(p)
}

/// `{privateServerInviteData | experienceInviteData}` → (placeId, linkCode?).
pub fn map_share(v: &Value) -> Option<(u64, Option<String>)> {
    let place = |d: &Value| d.get("placeId").and_then(Value::as_u64).filter(|&p| p != 0);
    if let Some(d) = v.get("privateServerInviteData").filter(|d| d.is_object()) {
        let code = d.get("linkCode").and_then(Value::as_str)?;
        return Some((place(d)?, Some(code.to_owned())));
    }
    let d = v.get("experienceInviteData").filter(|d| d.is_object())?;
    Some((place(d)?, None))
}

pub async fn share(
    c: &RobloxClient,
    code: &str,
    kind: ShareKind,
    cookie: &Secret,
) -> Result<(u64, Option<String>), String> {
    let t = match kind {
        ShareKind::Server => "Server",
        ShareKind::ExperienceInvite => "ExperienceInvite",
    };
    match games::resolve_share(c, code, t, cookie).await {
        Ok(v) => map_share(&v).ok_or_else(|| {
            log::info!("share link resolve: unexpected answer shape");
            SHARE_FAILED.to_owned()
        }),
        Err(e) => {
            log::info!("share link resolve failed: {e}");
            Err(SHARE_FAILED.into())
        }
    }
}

async fn follow_user(c: &RobloxClient, name: &str) -> Result<(u64, String), String> {
    let u = users::by_usernames(c, &[name.to_owned()])
        .await
        .map_err(|e| e.to_string())?;
    u.into_iter()
        .next()
        .map(|u| (u.id, u.name))
        .ok_or_else(|| format!("No Roblox user named {name}."))
}

fn place_preview(kind: &'static str, place_id: u64, r: Result<PlaceInfo, RobloxError>) -> Preview {
    match r {
        Ok(p) => Preview {
            kind,
            place_id: Some(place_id),
            universe_id: Some(p.universe_id),
            name: Some(p.name),
            icon: p.icon,
            playing: p.playing,
            ..Default::default()
        },
        Err(e) => {
            log::debug!("place {place_id}: {e}");
            Preview {
                kind,
                place_id: Some(place_id),
                // Only a transient failure gets its own text; the rest means "no such game".
                error: Some(match e {
                    RobloxError::Network(_) | RobloxError::RateLimited => e.to_string(),
                    _ => "Couldn't find that game.".into(),
                }),
                ..Default::default()
            }
        }
    }
}

/// Preview chip for the Play bar. `cookie` is any signed-in account (share links only).
pub async fn preview(c: &RobloxClient, input: &str, cookie: Option<&Secret>) -> Preview {
    let key = input.trim().to_owned();
    if let Some(p) = cached(&PREVIEWS, &key) {
        return p;
    }
    let p = match parse_destination(&key) {
        None => err("invalid", "Enter a place id, game link or @username."),
        Some(Destination::Query(_)) => Preview {
            kind: "query",
            ..Default::default()
        },
        Some(Destination::Place(id)) => place_preview("place", id, place_info(c, id).await),
        Some(Destination::Private {
            place_id,
            link_code,
        }) => Preview {
            private_link_code: Some(link_code),
            ..place_preview("private", place_id, place_info(c, place_id).await)
        },
        Some(Destination::Job { place_id, job_id }) => Preview {
            job_id: Some(job_id),
            ..place_preview("job", place_id, place_info(c, place_id).await)
        },
        Some(Destination::Follow(name)) => match follow_user(c, &name).await {
            Ok((user_id, username)) => {
                let presence = presence::get(c, &[user_id], None)
                    .await
                    .ok()
                    .and_then(|v| v.into_iter().next())
                    .map(|p| crate::accounts::map_presence(&p, None, &crate::accounts::now_iso()));
                Preview {
                    kind: "follow",
                    follow: Some(Follow {
                        user_id,
                        username,
                        presence,
                    }),
                    ..Default::default()
                }
            }
            Err(e) => err("follow", e),
        },
        Some(Destination::Share { code, kind }) => match cookie {
            None => err("share", "Sign in to an account to resolve share links."),
            Some(ck) => match share(c, &code, kind, ck).await {
                Ok((place_id, link)) => Preview {
                    private_link_code: link,
                    ..place_preview("share", place_id, place_info(c, place_id).await)
                },
                Err(e) => err("share", e),
            },
        },
    };
    // Only clean answers are cached; follow presence goes stale fast.
    if p.error.is_none() && p.kind != "follow" {
        store(&PREVIEWS, key, p.clone(), TTL);
    }
    p
}

/// Saved per-account target beats the bar unless "use bar for all".
pub fn effective_bar(
    t: Option<&crate::accounts::LaunchTarget>,
    bar: &Bar,
    use_bar_for_all: bool,
) -> Bar {
    let Some(t) = t.filter(|_| !use_bar_for_all) else {
        return bar.clone();
    };
    let dest = t.place_id.map(|p| p.to_string()).unwrap_or_default();
    match (&t.private_link, &t.job_id, t.place_id) {
        (Some(l), _, _) => Bar {
            destination: dest,
            server: l.clone(),
            mode: Mode::Specific,
            ..Default::default()
        },
        (None, Some(j), Some(_)) => Bar {
            destination: dest,
            server: j.clone(),
            mode: Mode::Specific,
            ..Default::default()
        },
        (None, None, Some(_)) => Bar {
            destination: dest,
            mode: match t.mode {
                Some(m @ (Mode::Random | Mode::Smallest)) => m,
                _ => Mode::Any,
            },
            ..Default::default()
        },
        _ => bar.clone(),
    }
}

/// Random: any non-full server. Smallest: fewest players, not empty, not full.
pub fn pick_server(s: &[servers::Server], mode: Mode) -> Option<&servers::Server> {
    let open = s
        .iter()
        .filter(|s| s.id.is_some() && s.playing < s.max_players);
    match mode {
        Mode::Random => {
            let v: Vec<_> = open.collect();
            (!v.is_empty()).then(|| v[rand::random_range(0..v.len())])
        }
        Mode::Smallest => open.filter(|s| s.playing > 0).min_by_key(|s| s.playing),
        _ => None,
    }
}

async fn scan(c: &RobloxClient, place_id: u64, mode: Mode, pages: u32) -> Result<String, String> {
    let mut all = vec![];
    let mut cursor: Option<String> = None;
    for _ in 0..pages.max(1) {
        let (s, next) = servers::page(c, place_id, servers::Kind::Public, cursor.as_deref(), None)
            .await
            .map_err(|e| e.to_string())?;
        all.extend(s);
        match next {
            Some(n) if !n.is_empty() => cursor = Some(n),
            _ => break,
        }
    }
    pick_server(&all, mode)
        .and_then(|s| s.id.clone())
        .ok_or_else(|| "No open server found. Try Any server.".into())
}

/// Bar → join request (+ the place id for recents; none when following). Empty bar with
/// `home` → `(None, None)`: no game, Roblox opens on its home screen.
pub async fn join_request(
    c: &RobloxClient,
    bar: &Bar,
    cookie: &Secret,
    recents: &[Recent],
    pages: u32,
) -> Result<(Option<JoinRequest>, Option<u64>), String> {
    if bar.home && bar.destination.trim().is_empty() && bar.server.trim().is_empty() {
        return Ok((None, None));
    }
    let (r, p) = game_request(c, bar, cookie, recents, pages).await?;
    Ok((Some(r), p))
}

async fn game_request(
    c: &RobloxClient,
    bar: &Bar,
    cookie: &Secret,
    recents: &[Recent],
    pages: u32,
) -> Result<(JoinRequest, Option<u64>), String> {
    let server = parse_server(&bar.server);
    if bar.mode == Mode::Specific {
        if let Some(ServerTarget::PrivateLink {
            place_id,
            link_code,
        }) = server
        {
            let code = PrivateCode::Link(link_code);
            return Ok((JoinRequest::Private { place_id, code }, Some(place_id)));
        }
        // A game-instance link names its place too, like a private server link.
        if let Some(Destination::Job { place_id, job_id }) = parse_destination(&bar.server) {
            return Ok((JoinRequest::Job { place_id, job_id }, Some(place_id)));
        }
        // A share link in the server field (or a saved privateLink) is resolved like the
        // destination field would be.
        if let Some(Destination::Share { code, kind }) = parse_destination(&bar.server) {
            return Ok(match share(c, &code, kind, cookie).await? {
                (place_id, Some(link)) => {
                    let code = PrivateCode::Link(link);
                    (JoinRequest::Private { place_id, code }, Some(place_id))
                }
                (place_id, None) => (JoinRequest::Place { place_id }, Some(place_id)),
            });
        }
    }
    let place_id = match parse_destination(&bar.destination) {
        None => return Err("Enter a destination first.".into()),
        Some(Destination::Place(p)) => p,
        Some(Destination::Private {
            place_id,
            link_code,
        }) => {
            let code = PrivateCode::Link(link_code);
            return Ok((JoinRequest::Private { place_id, code }, Some(place_id)));
        }
        Some(Destination::Job { place_id, job_id }) => {
            return Ok((JoinRequest::Job { place_id, job_id }, Some(place_id)))
        }
        Some(Destination::Follow(name)) => {
            let (user_id, _) = follow_user(c, &name).await?;
            return Ok((JoinRequest::Follow { user_id }, None));
        }
        Some(Destination::Share { code, kind }) => match share(c, &code, kind, cookie).await? {
            (place_id, Some(link)) => {
                let code = PrivateCode::Link(link);
                return Ok((JoinRequest::Private { place_id, code }, Some(place_id)));
            }
            (p, None) => p,
        },
        Some(Destination::Query(q)) => recents
            .iter()
            .find(|r| r.name.eq_ignore_ascii_case(q.trim()))
            .map(|r| r.place_id)
            .ok_or("Pick a game from the list or paste a link.")?,
    };
    let req = match bar.mode {
        Mode::Any => JoinRequest::Place { place_id },
        Mode::Specific => match server {
            Some(ServerTarget::JobId(job_id)) => JoinRequest::Job { place_id, job_id },
            _ => return Err("Enter a server JobId or private server link.".into()),
        },
        m => JoinRequest::Job {
            place_id,
            job_id: scan(c, place_id, m, pages).await?,
        },
    };
    Ok((req, Some(place_id)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::LaunchTarget;
    use serde_json::json;

    fn srv(id: &str, playing: u32, max: u32) -> servers::Server {
        servers::Server {
            id: Some(id.into()),
            playing,
            max_players: max,
            ..Default::default()
        }
    }

    #[test]
    fn smallest_skips_empty_and_full() {
        let s = [
            srv("full", 10, 10),
            srv("empty", 0, 10),
            srv("b", 5, 10),
            srv("a", 2, 10),
        ];
        assert_eq!(
            pick_server(&s, Mode::Smallest).unwrap().id.as_deref(),
            Some("a")
        );
        assert!(pick_server(&[srv("e", 0, 5)], Mode::Smallest).is_none());
    }

    #[test]
    fn random_only_non_full() {
        let s = [srv("full", 10, 10), srv("ok", 3, 10)];
        for _ in 0..20 {
            assert_eq!(
                pick_server(&s, Mode::Random).unwrap().id.as_deref(),
                Some("ok")
            );
        }
        assert!(pick_server(&[srv("full", 1, 1)], Mode::Random).is_none());
    }

    #[test]
    fn saved_target_beats_bar_unless_use_bar_for_all() {
        let bar = Bar {
            destination: "1".into(),
            server: String::new(),
            mode: Mode::Random,
            ..Default::default()
        };
        let t = LaunchTarget {
            place_id: Some(9),
            job_id: Some("j".into()),
            private_link: None,
            mode: None,
        };
        let e = effective_bar(Some(&t), &bar, false);
        assert_eq!(
            (e.destination.as_str(), e.server.as_str(), e.mode),
            ("9", "j", Mode::Specific)
        );
        assert_eq!(effective_bar(Some(&t), &bar, true), bar);
        assert_eq!(effective_bar(None, &bar, false), bar);
        let link = LaunchTarget {
            private_link: Some("https://www.roblox.com/games/9?privateServerLinkCode=L".into()),
            ..Default::default()
        };
        assert_eq!(effective_bar(Some(&link), &bar, false).mode, Mode::Specific);
        // an empty saved target means "no saved target"
        assert_eq!(
            effective_bar(Some(&LaunchTarget::default()), &bar, false),
            bar
        );
    }

    #[test]
    fn account_then_group_then_bar() {
        use crate::accounts::{target_for, Account};
        let bar = Bar {
            destination: "1".into(),
            ..Default::default()
        };
        let mut p = crate::vault::model::VaultPayload::default();
        let mut a = Account {
            group: "RC".into(),
            ..Default::default()
        };
        p.accounts.push(a.clone());
        let run = |p: &crate::vault::model::VaultPayload, a: &Account, all| {
            effective_bar(target_for(p, a).as_ref(), &bar, all)
        };
        assert_eq!(run(&p, &a, false), bar);
        let g = LaunchTarget {
            private_link: Some("https://www.roblox.com/share?code=G&type=Server".into()),
            ..Default::default()
        };
        p.group_targets.insert("RC".into(), g);
        assert_eq!(run(&p, &a, false).mode, Mode::Specific);
        assert_eq!(run(&p, &a, true), bar);
        // an empty own target does not hide the group's
        a.launch_target = Some(LaunchTarget::default());
        assert_eq!(run(&p, &a, false).mode, Mode::Specific);
        a.launch_target = Some(LaunchTarget {
            place_id: Some(5),
            ..Default::default()
        });
        let e = run(&p, &a, false);
        assert_eq!((e.destination.as_str(), e.mode), ("5", Mode::default()));
        assert_eq!(run(&p, &a, true), bar);
    }

    #[test]
    fn saved_mode_picks_server() {
        let t = LaunchTarget {
            place_id: Some(5),
            mode: Some(Mode::Smallest),
            ..Default::default()
        };
        let e = effective_bar(Some(&t), &Bar::default(), false);
        assert_eq!((e.destination.as_str(), e.mode), ("5", Mode::Smallest));
    }

    #[test]
    fn share_answer_mapping() {
        let ps = json!({"privateServerInviteData": {"placeId": 5, "linkCode": "LC", "status": "Valid"}, "experienceInviteData": null});
        assert_eq!(map_share(&ps), Some((5, Some("LC".into()))));
        let ex = json!({"privateServerInviteData": null, "experienceInviteData": {"placeId": 6}});
        assert_eq!(map_share(&ex), Some((6, None)));
        assert_eq!(map_share(&json!({})), None);
        assert_eq!(
            map_share(&json!({"privateServerInviteData": {"placeId": 5}})),
            None
        );
    }

    #[tokio::test]
    async fn bar_to_join_request_without_network() {
        let c = RobloxClient::new(crate::roblox::BaseUrls::all("http://127.0.0.1:9"));
        let ck = Secret::new("x");
        let rec = [Recent {
            place_id: 77,
            name: "Doors".into(),
            thumbnail: None,
            last_played_at: String::new(),
        }];
        let jr = |d: &str, s: &str, m: Mode| Bar {
            destination: d.into(),
            server: s.into(),
            mode: m,
            home: true,
        };
        let j = "11111111-2222-3333-4444-555555555555";
        let (r, p) = join_request(&c, &jr("doors", "", Mode::Any), &ck, &rec, 1)
            .await
            .unwrap();
        assert!(matches!(r, Some(JoinRequest::Place { place_id: 77 })) && p == Some(77));
        let (r, _) = join_request(&c, &jr("5", j, Mode::Specific), &ck, &rec, 1)
            .await
            .unwrap();
        assert!(matches!(r, Some(JoinRequest::Job { place_id: 5, .. })));
        let link = "https://www.roblox.com/games/8/x?privateServerLinkCode=LC";
        let (r, p) = join_request(&c, &jr("", link, Mode::Specific), &ck, &rec, 1)
            .await
            .unwrap();
        assert!(
            matches!(r, Some(JoinRequest::Private { place_id: 8, code: PrivateCode::Link(ref l) }) if l == "LC")
                && p == Some(8)
        );
        // a game-instance link in the server field brings its own place
        let inst = format!("https://www.roblox.com/games/9/x?gameInstanceId={j}");
        let (r, p) = join_request(&c, &jr("", &inst, Mode::Specific), &ck, &rec, 1)
            .await
            .unwrap();
        assert!(matches!(r, Some(JoinRequest::Job { place_id: 9, .. })) && p == Some(9));
        assert!(join_request(&c, &jr("5", "", Mode::Specific), &ck, &rec, 1)
            .await
            .is_err());
        // no game at all: Roblox home, whatever the mode
        for m in [Mode::Any, Mode::Specific, Mode::Random] {
            let (r, p) = join_request(&c, &jr(" ", "", m), &ck, &rec, 1)
                .await
                .unwrap();
            assert!(r.is_none() && p.is_none());
        }
        // without the opt-in (automation, auto-reconnect) an empty bar still fails
        let empty = Bar::default();
        assert_eq!(
            join_request(&c, &empty, &ck, &rec, 1)
                .await
                .err()
                .as_deref(),
            Some("Enter a destination first.")
        );
        // a server but no game still needs the game
        assert!(join_request(&c, &jr("", j, Mode::Specific), &ck, &rec, 1)
            .await
            .is_err());
        assert!(join_request(&c, &jr("nope", "", Mode::Any), &ck, &rec, 1)
            .await
            .is_err());
    }

    #[test]
    fn cache_honors_ttl_and_store_drops_expired() {
        let c: Cache<u32> = LazyLock::new(Default::default);
        store(&c, "a".into(), 1, Duration::ZERO);
        assert_eq!(cached(&c, "a"), None, "expired at once");
        store(&c, "b".into(), 2, Duration::from_secs(3600));
        assert_eq!(cached(&c, "b"), Some(2));
        assert_eq!(c.lock().unwrap().len(), 1, "store drops expired entries");
    }

    #[test]
    fn place_errors_show_transient_text_only() {
        let info = PlaceInfo {
            universe_id: 3,
            name: "Doors".into(),
            icon: Some("i".into()),
            playing: Some(4),
        };
        let ok = place_preview("place", 5, Ok(info));
        assert_eq!(
            (ok.universe_id, ok.name.as_deref(), ok.playing, ok.error),
            (Some(3), Some("Doors"), Some(4), None)
        );
        let net = RobloxError::Network("down".into());
        let p = place_preview("place", 5, Err(RobloxError::Network("down".into())));
        assert_eq!(p.error, Some(net.to_string()));
        let rl = place_preview("place", 5, Err(RobloxError::RateLimited));
        assert_eq!(rl.error, Some(RobloxError::RateLimited.to_string()));
        for e in [
            RobloxError::Decode("x".into()),
            RobloxError::Unauthorized,
            RobloxError::Message("m".into()),
        ] {
            let p = place_preview("place", 5, Err(e));
            assert_eq!(p.error.as_deref(), Some("Couldn't find that game."));
            assert_eq!(p.place_id, Some(5));
        }
    }

    #[test]
    fn pick_server_skips_no_id_and_any_mode_picks_nothing() {
        let ghost = || servers::Server {
            id: None,
            playing: 1,
            max_players: 10,
            ..Default::default()
        };
        assert!(pick_server(&[ghost()], Mode::Smallest).is_none());
        assert!(pick_server(&[ghost()], Mode::Random).is_none());
        assert!(pick_server(&[srv("a", 1, 10)], Mode::Any).is_none());
    }

    #[test]
    fn map_share_rejects_place_zero_and_non_object_invites() {
        assert_eq!(
            map_share(&json!({"privateServerInviteData": {"placeId": 0, "linkCode": "LC"}})),
            None
        );
        assert_eq!(
            map_share(&json!({"experienceInviteData": {"placeId": 0}})),
            None
        );
        assert_eq!(map_share(&json!({"experienceInviteData": "x"})), None);
    }

    #[test]
    fn saved_job_without_place_and_saved_mode_rules() {
        let bar = Bar {
            destination: "1".into(),
            ..Default::default()
        };
        let job_only = LaunchTarget {
            job_id: Some("j".into()),
            ..Default::default()
        };
        assert_eq!(effective_bar(Some(&job_only), &bar, false), bar);
        let saved = |m: Option<Mode>| LaunchTarget {
            place_id: Some(5),
            mode: m,
            ..Default::default()
        };
        assert_eq!(
            effective_bar(Some(&saved(Some(Mode::Specific))), &bar, false).mode,
            Mode::Any
        );
        assert_eq!(
            effective_bar(Some(&saved(Some(Mode::Random))), &bar, false).mode,
            Mode::Random
        );
    }

    #[tokio::test]
    async fn preview_answers_without_network() {
        let c = RobloxClient::new(crate::roblox::BaseUrls::all("http://127.0.0.1:9"));
        let p = preview(&c, "  ", None).await;
        assert_eq!((p.kind, p.error.is_some()), ("invalid", true));
        assert_eq!(preview(&c, "adopt me", None).await.kind, "query");
        let share = preview(&c, "https://www.roblox.com/share?code=a&type=Server", None).await;
        assert_eq!(
            share,
            err("share", "Sign in to an account to resolve share links.")
        );
    }

    #[tokio::test]
    async fn specific_mode_resolves_share_in_server_field() {
        let c = RobloxClient::new(crate::roblox::BaseUrls::all("http://127.0.0.1:9"));
        let bar = Bar {
            destination: "5".into(),
            server: "https://www.roblox.com/share?code=abc&type=Server".into(),
            mode: Mode::Specific,
            home: false,
        };
        let Err(e) = join_request(&c, &bar, &Secret::new("x"), &[], 1).await else {
            panic!("share must go through resolve, which fails offline");
        };
        assert_eq!(e, SHARE_FAILED);
    }
}

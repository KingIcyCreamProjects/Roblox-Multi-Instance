//! Play-bar destination / server-field parsers. Pure, no network.

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ShareKind {
    Server,
    ExperienceInvite,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Destination {
    Place(u64),
    Private { place_id: u64, link_code: String },
    Share { code: String, kind: ShareKind },
    Follow(String),
    Job { place_id: u64, job_id: String },
    Query(String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum ServerTarget {
    JobId(String),
    PrivateLink { place_id: u64, link_code: String },
}

fn is_uuid(s: &str) -> bool {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(
            r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$",
        )
        .unwrap()
    })
    .is_match(s)
}

/// Nonzero u64 or nothing.
fn id(s: &str) -> Option<u64> {
    s.parse().ok().filter(|&n| n != 0)
}

/// Roblox link -> destination. `Some(None)` = it is a Roblox games link with a bad id.
fn parse_link(s: &str, depth: u8) -> Option<Option<Destination>> {
    // A scheme only counts before the path/query: `roblox.com/games/5?a=https://b` has none.
    let full = if s
        .find("://")
        .is_some_and(|i| !s[..i].contains(['/', '?', '#']))
    {
        s.to_string()
    } else {
        format!("https://{s}")
    };
    let u = url::Url::parse(&full).ok()?;
    if !matches!(u.scheme(), "http" | "https") {
        return None;
    }
    let host = u.host_str()?.to_ascii_lowercase();
    if !(host == "roblox.com" || host.ends_with(".roblox.com") || host == "ro.blox.com") {
        return None;
    }
    let q = |k: &str| {
        u.query_pairs()
            .find(|(n, v)| n == k && !v.is_empty())
            .map(|(_, v)| v.into_owned())
    };
    // ro.blox.com redirect links carry the real target in af_web_dp (depth 1 only)
    if host == "ro.blox.com" && depth == 0 {
        if let Some(t) = q("af_web_dp") {
            return parse_link(&t, 1);
        }
    }
    let segs: Vec<&str> = u.path_segments()?.collect();
    // optional locale prefix: /de/..., /pt-br/...
    let is_locale = |l: &str| {
        let b = l.as_bytes();
        let lower = |c: &u8| c.is_ascii_lowercase();
        (b.len() == 2 && b.iter().all(lower))
            || (b.len() == 5 && b[2] == 45 && b[..2].iter().all(lower) && b[3..].iter().all(lower))
    };
    let segs = match segs.get(1) {
        Some(&("games" | "share" | "share-links")) if is_locale(segs[0]) => &segs[1..],
        _ => &segs[..],
    };
    match segs {
        ["games", pid, ..] => {
            // Roblox's own deeplink: /games/start?placeId=N
            let start = || {
                q("placeId")
                    .and_then(|p| id(&p))
                    .filter(|_| *pid == "start")
            };
            let Some(place_id) = id(pid).or_else(start) else {
                return Some(None);
            };
            Some(Some(if let Some(link_code) = q("privateServerLinkCode") {
                Destination::Private {
                    place_id,
                    link_code,
                }
            } else if let Some(job_id) = q("gameInstanceId").filter(|j| is_uuid(j)) {
                Destination::Job { place_id, job_id }
            } else {
                Destination::Place(place_id)
            }))
        }
        ["share" | "share-links", ..] => {
            let kind = match q("type")?.as_str() {
                "Server" => ShareKind::Server,
                "ExperienceInvite" => ShareKind::ExperienceInvite,
                _ => return None,
            };
            Some(Some(Destination::Share {
                code: q("code")?,
                kind,
            }))
        }
        _ => None,
    }
}

pub fn parse_destination(input: &str) -> Option<Destination> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(name) = s.strip_prefix('@') {
        let ok = (3..=20).contains(&name.len())
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
        return Some(if ok {
            Destination::Follow(name.to_string())
        } else {
            Destination::Query(s.to_string())
        });
    }
    if s.bytes().all(|b| b.is_ascii_digit()) {
        return id(s).map(Destination::Place);
    }
    match parse_link(s, 0) {
        Some(d) => d,
        None => Some(Destination::Query(s.to_string())),
    }
}

pub fn parse_server(input: &str) -> Option<ServerTarget> {
    let s = input.trim();
    if is_uuid(s) {
        return Some(ServerTarget::JobId(s.to_string()));
    }
    match parse_destination(s)? {
        Destination::Private {
            place_id,
            link_code,
        } => Some(ServerTarget::PrivateLink {
            place_id,
            link_code,
        }),
        // a game-instance link names a server too
        Destination::Job { job_id, .. } => Some(ServerTarget::JobId(job_id)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Destination::*;

    const JOB: &str = "0a1b2c3d-1111-4222-8333-444455556666";

    fn p(s: &str) -> Option<Destination> {
        parse_destination(s)
    }
    fn private(id: u64, c: &str) -> Option<Destination> {
        Some(Private {
            place_id: id,
            link_code: c.into(),
        })
    }

    #[test]
    fn empty() {
        assert_eq!(p(""), None);
        assert_eq!(p("  \t\n"), None);
        assert_eq!(parse_server("  "), None);
    }

    #[test]
    fn bare_id() {
        assert_eq!(p("920587237"), Some(Place(920587237)));
        assert_eq!(p("  920587237 \n"), Some(Place(920587237)));
    }

    #[test]
    fn absurd_ids() {
        assert_eq!(p("0"), None);
        assert_eq!(p("99999999999999999999999"), None);
        assert_eq!(p("roblox.com/games/0/x"), None);
        assert_eq!(p("roblox.com/games/99999999999999999999999/x"), None);
    }

    #[test]
    fn game_urls() {
        for s in [
            "https://www.roblox.com/games/920587237/Adopt-Me",
            "http://roblox.com/games/920587237/Adopt-Me",
            "roblox.com/games/920587237/Adopt-Me",
            "www.roblox.com/games/920587237",
            "web.roblox.com/games/920587237/x?foo=bar#frag",
            "https://www.roblox.com/de/games/920587237/Adopt-Me",
            "https://www.roblox.com/pt-br/games/920587237/x  ",
            "  https://www.roblox.com/games/920587237/Adopt-Me\n",
            "roblox.com/games/920587237/x?a=https://b",
            "https://www.roblox.com/games/start?placeId=920587237&launchData=x",
        ] {
            assert_eq!(p(s), Some(Place(920587237)), "{s}");
        }
        assert_eq!(p("roblox.com/games/start"), None);
        assert_eq!(p("roblox.com/games/x?placeId=5"), None);
    }

    #[test]
    fn private_links() {
        assert_eq!(
            p("https://www.roblox.com/games/920587237/Adopt-Me?privateServerLinkCode=1234567890"),
            private(920587237, "1234567890")
        );
        assert_eq!(
            p("roblox.com/de/games/5/x?a=b&privateServerLinkCode=zz "),
            private(5, "zz")
        );
    }

    #[test]
    fn game_instance_url_is_job() {
        assert_eq!(
            p(&format!(
                "https://www.roblox.com/games/5/x?gameInstanceId={JOB}"
            )),
            Some(Job {
                place_id: 5,
                job_id: JOB.into()
            })
        );
    }

    #[test]
    fn share_links() {
        let s = |c: &str, k| {
            Some(Share {
                code: c.into(),
                kind: k,
            })
        };
        let (srv, inv) = (ShareKind::Server, ShareKind::ExperienceInvite);
        assert_eq!(
            p("https://www.roblox.com/share?code=abc123&type=Server"),
            s("abc123", srv)
        );
        assert_eq!(
            p("roblox.com/share?code=abc123&type=ExperienceInvite"),
            s("abc123", inv)
        );
        assert_eq!(
            p("https://www.roblox.com/share?code=abc&type=Server&stamp=1"),
            s("abc", srv)
        );
        assert_eq!(
            p("https://www.roblox.com/share-links?code=abc&type=Server"),
            s("abc", srv)
        );
        assert_eq!(
            p("https://ro.blox.com/share?code=abc&type=ExperienceInvite"),
            s("abc", inv)
        );
    }

    #[test]
    fn ro_blox_redirect() {
        let u = "https://ro.blox.com/Ebh5?pid=share&af_web_dp=https%3A%2F%2Fwww.roblox.com%2Fshare-links%3Fcode%3Dabc123%26type%3DServer";
        assert_eq!(
            p(u),
            Some(Share {
                code: "abc123".into(),
                kind: ShareKind::Server
            })
        );
        // nested redirect not followed
        let n = "https://ro.blox.com/a?af_web_dp=https%3A%2F%2Fro.blox.com%2Fb%3Faf_web_dp%3Dhttps%253A%252F%252Fwww.roblox.com%252Fgames%252F5";
        assert_eq!(p(n), Some(Query(n.into())));
        // redirect to foreign host
        let f = "https://ro.blox.com/a?af_web_dp=https%3A%2F%2Fevil.com%2Fgames%2F5";
        assert_eq!(p(f), Some(Query(f.into())));
    }

    #[test]
    fn instance_id_must_be_uuid() {
        assert_eq!(
            p("roblox.com/games/5/x?gameInstanceId=nope"),
            Some(Place(5))
        );
    }

    #[test]
    fn locale_only_xx_or_xx_yy() {
        assert_eq!(p("roblox.com/de/games/5"), Some(Place(5)));
        assert_eq!(p("roblox.com/pt-br/games/5"), Some(Place(5)));
        for bad in [
            "roblox.com/deu/games/5",
            "roblox.com/d1/games/5",
            "roblox.com/pt_br/games/5",
            "roblox.com/PT-br/games/5",
        ] {
            assert_eq!(p(bad), Some(Query(bad.into())), "{bad}");
        }
    }

    #[test]
    fn hostile_inputs() {
        for bad in [
            "https://roblox.com@evil.com/games/5",
            "https://evil.com/roblox.com/games/5",
            "https://roblox.com.evil.com/games/5",
            "https://notroblox.com/games/5",
            "https://evilroblox.com/games/5",
            "roblox.com.evil.com/games/5",
            "javascript:alert(1)//roblox.com/games/5",
            "file:///C:/roblox.com/games/5",
            "ftp://roblox.com/games/5",
            "data:text/html,roblox.com/games/5",
            "https://rоblox.com/games/5",
            "https://xn--rblox-5cf.com/games/5",
            "https://roblox.com:99999/games/5",
            "<script>alert(1)</script>",
        ] {
            assert_eq!(p(bad), Some(Query(bad.into())), "{bad}");
        }
        let huge = "9".repeat(5000);
        assert_eq!(p(&huge), None);
        assert_eq!(p(&format!("roblox.com/games/{huge}")), None);
        assert_eq!(parse_server("@x"), None);
        assert_eq!(parse_server(&huge), None);
    }

    #[test]
    fn share_bad_type_is_query() {
        let u = "https://www.roblox.com/share?code=abc&type=Nope";
        assert_eq!(p(u), Some(Query(u.into())));
    }

    #[test]
    fn follow() {
        assert_eq!(p("@Builderman"), Some(Follow("Builderman".into())));
        assert_eq!(p("  @a_b "), Some(Follow("a_b".into())));
        for bad in [
            "@",
            "@ab",
            "@a b",
            "@<script>",
            "@ x",
            "@aaaaaaaaaaaaaaaaaaaaa",
            "@Ünï",
        ] {
            assert_eq!(p(bad), Some(Query(bad.into())), "{bad}");
        }
    }

    #[test]
    fn query_fallback() {
        assert_eq!(p("adopt me "), Some(Query("adopt me".into())));
        let u = "https://example.com/games/5";
        assert_eq!(p(u), Some(Query(u.into())));
        // a lone job id is not a destination
        assert_eq!(p(JOB), Some(Query(JOB.into())));
    }

    #[test]
    fn server_field() {
        assert_eq!(
            parse_server(&format!(" {JOB} ")),
            Some(ServerTarget::JobId(JOB.into()))
        );
        assert_eq!(
            parse_server("roblox.com/games/9/x?privateServerLinkCode=qq"),
            Some(ServerTarget::PrivateLink {
                place_id: 9,
                link_code: "qq".into()
            })
        );
        assert_eq!(parse_server("hello"), None);
        assert_eq!(parse_server("920587237"), None);
    }

    #[test]
    fn share_missing_or_empty_fields_are_query() {
        for bad in [
            "https://www.roblox.com/share?type=Server",
            "https://www.roblox.com/share?code=&type=Server",
            "https://www.roblox.com/share?code=abc",
            "https://www.roblox.com/share?code=abc&type=",
        ] {
            assert_eq!(p(bad), Some(Query(bad.into())), "{bad}");
        }
    }

    #[test]
    fn empty_params_fall_back_and_private_wins_over_job() {
        assert_eq!(
            p("roblox.com/games/5/x?privateServerLinkCode="),
            Some(Place(5))
        );
        assert_eq!(p("roblox.com/games/5/x?gameInstanceId="), Some(Place(5)));
        assert_eq!(
            p(&format!(
                "roblox.com/games/5/x?gameInstanceId={JOB}&privateServerLinkCode=LC"
            )),
            private(5, "LC")
        );
    }

    #[test]
    fn locale_share_host_case_and_other_paths() {
        let share = Some(Share {
            code: "abc".into(),
            kind: ShareKind::Server,
        });
        assert_eq!(p("roblox.com/de/share?code=abc&type=Server"), share);
        assert_eq!(p("https://WWW.ROBLOX.COM/games/5"), Some(Place(5)));
        // not a locale: two-letter prefix needed
        assert_eq!(
            p("roblox.com/x/games/5"),
            Some(Query("roblox.com/x/games/5".into()))
        );
        assert_eq!(
            p("roblox.com/users/5"),
            Some(Query("roblox.com/users/5".into()))
        );
    }

    #[test]
    fn ro_blox_plain_and_empty_redirect() {
        assert_eq!(p("https://ro.blox.com/games/5"), Some(Place(5)));
        let plain = "https://ro.blox.com/Ebh5";
        assert_eq!(p(plain), Some(Query(plain.into())));
        let empty = "https://ro.blox.com/Ebh5?af_web_dp=";
        assert_eq!(p(empty), Some(Query(empty.into())));
    }

    #[test]
    fn follow_max_length_accepted() {
        let max = format!("@{}", "a".repeat(20));
        assert_eq!(p(&max), Some(Follow("a".repeat(20))));
    }

    #[test]
    fn server_uuid_case_and_non_targets() {
        let up = JOB.to_uppercase();
        assert_eq!(parse_server(&up), Some(ServerTarget::JobId(up)));
        assert_eq!(parse_server("0a1b2c3d-1111-4222-8333-44445555666"), None);
        assert_eq!(
            parse_server("https://www.roblox.com/share?code=a&type=Server"),
            None
        );
        assert_eq!(parse_server("roblox.com/games/5/x"), None);
    }

    #[test]
    fn server_field_takes_a_job_link() {
        assert_eq!(
            parse_server(&format!(
                "https://www.roblox.com/games/5/x?gameInstanceId={JOB}"
            )),
            Some(ServerTarget::JobId(JOB.into()))
        );
    }
}

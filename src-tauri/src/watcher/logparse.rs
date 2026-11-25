//! Roblox client log -> server connection state, driven by the data file `rules.json`.

use regex::RegexSet;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Srv {
    /// No matching line seen (yet).
    #[default]
    Unknown,
    Joined,
    Left,
    Disconnected,
}

#[derive(Deserialize)]
struct Raw {
    version: u32,
    joined: Vec<String>,
    left: Vec<String>,
    disconnected: Vec<String>,
}

pub struct LogRules {
    joined: RegexSet,
    left: RegexSet,
    disconnected: RegexSet,
}

const EMBEDDED: &str = include_str!("rules.json");

fn set(v: &[String]) -> Result<RegexSet, String> {
    regex::RegexSetBuilder::new(v)
        .case_insensitive(true)
        .build()
        .map_err(|e| e.to_string())
}

impl LogRules {
    pub fn from_json(s: &str) -> Result<Self, String> {
        let r: Raw = serde_json::from_str(s).map_err(|e| e.to_string())?;
        if r.version != 1 {
            return Err(format!("unsupported watcher rules version {}", r.version));
        }
        Ok(Self {
            joined: set(&r.joined)?,
            left: set(&r.left)?,
            disconnected: set(&r.disconnected)?,
        })
    }

    pub fn embedded() -> Self {
        Self::from_json(EMBEDDED).expect("embedded rules.json is valid")
    }

    /// `<data>/watcher-rules.json` when present and valid, else the embedded file.
    /// Returns the rules and a label of which was used.
    pub fn load(data_dir: &std::path::Path) -> (Self, &'static str) {
        let p = data_dir.join("watcher-rules.json");
        match std::fs::read_to_string(&p) {
            Ok(s) => match Self::from_json(&s) {
                Ok(r) => return (r, "user file"),
                Err(e) => log::warn!("{} invalid ({e}); using embedded rules", p.display()),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => log::warn!("{} unreadable ({e}); using embedded rules", p.display()),
        }
        (Self::embedded(), "embedded")
    }

    pub fn classify(&self, line: &str) -> Option<Srv> {
        if self.disconnected.is_match(line) {
            Some(Srv::Disconnected)
        } else if self.left.is_match(line) {
            Some(Srv::Left)
        } else if self.joined.is_match(line) {
            Some(Srv::Joined)
        } else {
            None
        }
    }

    /// Fold `text` onto `prev`: the last matching line wins.
    pub fn state_of(&self, text: &str, prev: Srv) -> Srv {
        text.lines()
            .filter_map(|l| self.classify(l))
            .next_back()
            .unwrap_or(prev)
    }
}

/// For auto reconnect: the last join (`place`, `jobId`) in `text`, and the last disconnect
/// reason code sent after it (`[FLog::Network] Sending disconnect with reason: <n>`; a later
/// join clears it), and whether `text` shows a non-public join: private server, reserved
/// server or teleport (Bloxstrap's markers). Hardcoded, not in rules.json: it needs
/// capture groups.
pub fn rejoin_events(text: &str) -> (Option<(u64, String)>, Option<u32>, bool) {
    use regex::Regex;
    use std::sync::OnceLock;
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(concat!(
            r"\[FLog::Output\] ! Joining game '([0-9a-f-]+)' place (\d+) at ",
            r"|\[FLog::Network\] Sending disconnect with reason: (\d+)",
            // Client 0.747 logs no code for a dropped connection, only this text.
            r"|\[FLog::Network\] Lost connection with reason : (.*)",
            r"|GameJoinUtil::(?:joinGamePostPrivateServer|initiateTeleportToReservedServer)",
            r"|\[FLog::SingleSurfaceApp\] initiateTeleport"
        ))
        .unwrap()
    });
    let (mut join, mut reason, mut marked) = (None, None, false);
    for c in text.lines().filter_map(|l| re.captures(l)) {
        // Branch on the group that matched; a number that does not parse is ignored, never
        // read as a marker.
        if let (Some(j), Some(p)) = (c.get(1), c.get(2)) {
            if let Ok(p) = p.as_str().parse() {
                join = Some((p, j.as_str().to_owned()));
                reason = None;
            }
        } else if let Some(n) = c.get(3) {
            if let Ok(n) = n.as_str().parse() {
                reason = Some(n);
            }
        } else if let Some(t) = c.get(4) {
            // 264: joined from another device (longer backoff); anything else: dropped.
            reason = Some(if t.as_str().contains("another device") {
                264
            } else {
                277
            });
        } else {
            marked = true;
        }
    }
    (join, reason, marked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejoin_events_join_then_reason() {
        let send = |n: u32| {
            format!("2026-10-24T09:00:01.000Z,1.0,5f64,6 [FLog::Network] Sending disconnect with reason: {n}")
        };
        let job = "00000000-0000-4000-8000-000000000000";
        let t = [JOINING.to_owned(), ACCEPTED.into(), send(277)].join("\n");
        assert_eq!(rejoin_events(&t), (Some((1, job.into())), Some(277), false));
        // teleport: 285 then a new join clears the reason
        let t = [send(285), JOINING.into()].join("\n");
        assert_eq!(rejoin_events(&t), (Some((1, job.into())), None, false));
        assert_eq!(rejoin_events(&send(268)), (None, Some(268), false));
        assert_eq!(rejoin_events(CHAT_WS), (None, None, false));
        // Real client 0.747 lines: no reason code, text only.
        let lost = |why: &str| {
            format!("2026-11-14T09:46:36.978Z,27986.978516,5c0c,7,Warning [FLog::Network] Lost connection with reason : {why}")
        };
        let t = lost("Lost connection to the game server, please reconnect");
        assert_eq!(rejoin_events(&t), (None, Some(277), false));
        let t = lost(
            "This game has been disconnected because you have joined a game from another device",
        );
        assert_eq!(rejoin_events(&t), (None, Some(264), false));
        for m in [
            "2026-10-24T07:03:14.000Z,1.0,5f64,6 [FLog::GameJoinUtil] GameJoinUtil::joinGamePostPrivateServer: x",
            "2026-10-24T07:03:14.000Z,1.0,5f64,6 [FLog::GameJoinUtil] GameJoinUtil::initiateTeleportToReservedServer: x",
            "2026-10-24T07:03:14.000Z,1.0,5f64,6 [FLog::SingleSurfaceApp] initiateTeleport: x",
        ] {
            let t = [m, JOINING].join("\n");
            let want = (Some((1, job.into())), None, true);
            assert_eq!(rejoin_events(&t), want, "{m}");
        }
        // Overflowing numbers: ignored, not a private-server marker.
        let big = JOINING.replace("place 1 at", "place 99999999999999999999 at");
        assert_eq!(rejoin_events(&big), (None, None, false));
        let big = format!("{}9", send(u32::MAX));
        assert_eq!(rejoin_events(&big), (None, None, false));
    }

    // Real client 0.744 lines, IPs and ids sanitised.
    const JOINING: &str = "2026-10-24T07:03:15.490Z,89.490379,5f64,6 [FLog::Output] ! Joining game '00000000-0000-4000-8000-000000000000' place 1 at 10.0.0.1";
    const ACCEPTED: &str = "2026-10-24T07:03:15.661Z,89.661125,7608,6,Info [DFLog::NetworkClient] Connection accepted from 192.0.2.1|64071";
    const REPLICATOR: &str = "2026-10-24T07:03:15.664Z,89.664116,7608,7 [FLog::Network] Replicator created for player 192.0.2.1|64071";
    const CLIENT_DISC: &str =
        "2026-10-24T08:46:22.436Z,6276.436523,1cdc,6,Info [DFLog::NetworkClient] Client:Disconnect";
    const TRANSPORT: &str = "2026-10-24T08:46:22.479Z,6276.479004,5f64,6,Info [DFLog::RbxTransportDummyClient] Disconnected from server for reason: Player: 285 (DisconnectClientInitiated)";
    const LOST: &str =
        "2026-10-24T09:00:00.000Z,1.0,5f64,6 [FLog::Network] Connection lost: AckTimeout 1";
    const CHAT_WS: &str = "2026-10-24T07:01:47.719Z,1.719150,33c0,6 [DFLog::SignalRCoreError] ID: 1 Disconnected - Websocket error: HttpError: DnsResolve";
    const CHAT_STOP: &str =
        "2026-10-24T07:01:47.719Z,1.719150,33c0,6 [DFLog::SignalRCoreError] ID: 1 Disconnected - stop() called";

    fn r() -> LogRules {
        LogRules::embedded()
    }

    #[test]
    fn embedded_parses() {
        let _ = r();
    }

    #[test]
    fn rejects_bad_version() {
        let j = r#"{"version":2,"joined":[],"left":[],"disconnected":[]}"#;
        assert!(LogRules::from_json(j).is_err());
    }

    #[test]
    fn real_lines() {
        let r = r();
        for l in [JOINING, ACCEPTED, REPLICATOR] {
            assert_eq!(r.classify(l), Some(Srv::Joined), "{l}");
        }
        assert_eq!(r.classify(CLIENT_DISC), Some(Srv::Left));
        assert_eq!(r.classify(TRANSPORT), Some(Srv::Disconnected));
        assert_eq!(r.classify(LOST), Some(Srv::Disconnected));
    }

    #[test]
    fn chat_socket_is_ignored() {
        let r = r();
        assert_eq!(r.classify(CHAT_WS), None);
        assert_eq!(r.classify(CHAT_STOP), None);
    }

    #[test]
    fn teleport_sequence_ends_joined() {
        let t = [
            JOINING,
            ACCEPTED,
            CLIENT_DISC,
            TRANSPORT,
            JOINING,
            CLIENT_DISC,
            ACCEPTED,
        ]
        .join("\n");
        assert_eq!(r().state_of(&t, Srv::Unknown), Srv::Joined);
        let t = [ACCEPTED, CHAT_WS, CLIENT_DISC].join("\n");
        assert_eq!(r().state_of(&t, Srv::Unknown), Srv::Left);
    }

    #[test]
    fn no_match_keeps_prev() {
        assert_eq!(r().state_of("nothing\nhere", Srv::Joined), Srv::Joined);
    }

    #[test]
    fn load_prefers_valid_user_file() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(LogRules::load(d.path()).1, "embedded");
        let f = d.path().join("watcher-rules.json");
        std::fs::write(&f, "not json").unwrap();
        assert_eq!(LogRules::load(d.path()).1, "embedded");
        std::fs::write(
            &f,
            r#"{"version":1,"joined":["zzjoin"],"left":[],"disconnected":[]}"#,
        )
        .unwrap();
        let (r, src) = LogRules::load(d.path());
        assert_eq!(src, "user file");
        assert_eq!(r.classify("x zzjoin"), Some(Srv::Joined));
    }

    #[test]
    fn rejects_bad_rules() {
        let bad_regex = r#"{"version":1,"joined":["("],"left":[],"disconnected":[]}"#;
        assert!(LogRules::from_json(bad_regex).is_err());
        let missing = r#"{"version":1,"joined":[],"left":[]}"#;
        assert!(LogRules::from_json(missing).is_err());
    }

    #[test]
    fn check_order_disconnected_then_left_then_joined() {
        let all = r#"{"version":1,"joined":["zz"],"left":["zz"],"disconnected":["zz"]}"#;
        assert_eq!(
            LogRules::from_json(all).unwrap().classify("zz"),
            Some(Srv::Disconnected)
        );
        let no_disc = r#"{"version":1,"joined":["zz"],"left":["zz"],"disconnected":[]}"#;
        assert_eq!(
            LogRules::from_json(no_disc).unwrap().classify("zz"),
            Some(Srv::Left)
        );
    }
}

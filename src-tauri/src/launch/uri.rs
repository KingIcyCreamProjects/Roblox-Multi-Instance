//! `roblox-player:` launch URI + PlaceLauncher URL builders. Pure, no I/O.

pub enum PrivateCode {
    Link(String),
    Access(String),
}

pub enum JoinRequest {
    Place { place_id: u64 },
    Job { place_id: u64, job_id: String },
    Private { place_id: u64, code: PrivateCode },
    Follow { user_id: u64 },
}

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// For the outer URI: `+` is the field separator, so space must be %20 (literal + is already %2B).
fn enc_field(s: &str) -> String {
    enc(s).replace('+', "%20")
}

pub fn build_place_launcher_url(
    req: &JoinRequest,
    tracker_id: &str,
    join_attempt_id: &str,
) -> String {
    let mut q = String::from("https://www.roblox.com/Game/PlaceLauncher.ashx?");
    let t = enc(tracker_id);
    match req {
        JoinRequest::Place { place_id } => {
            q += &format!("request=RequestGame&browserTrackerId={t}&placeId={place_id}")
        }
        JoinRequest::Job { place_id, job_id } => {
            q += &format!(
                "request=RequestGameJob&browserTrackerId={t}&placeId={place_id}&gameId={}",
                enc(job_id)
            )
        }
        JoinRequest::Private { place_id, code } => {
            q += &format!("request=RequestPrivateGame&browserTrackerId={t}&placeId={place_id}");
            match code {
                PrivateCode::Link(c) => q += &format!("&linkCode={}", enc(c)),
                PrivateCode::Access(c) => q += &format!("&accessCode={}", enc(c)),
            }
        }
        JoinRequest::Follow { user_id } => {
            q += &format!("request=RequestFollowUser&browserTrackerId={t}&userId={user_id}")
        }
    }
    q + &format!(
        "&isPlayTogetherGame=false&joinAttemptId={}&joinAttemptOrigin=PlayButton",
        enc(join_attempt_id)
    )
}

/// `launcher_url: None` = no game: Roblox opens signed in on its home screen (`launchmode:app`).
pub fn build_launch_uri(
    ticket: &str,
    launch_time_ms: u64,
    tracker_id: &str,
    launcher_url: Option<&str>,
) -> String {
    let (mode, place) = match launcher_url {
        Some(u) => ("play", format!("+placelauncherurl:{}", enc_field(u))),
        None => ("app", String::new()),
    };
    format!(
        "roblox-player:1+launchmode:{mode}+gameinfo:{}+launchtime:{launch_time_ms}{place}+browsertrackerid:{}+robloxLocale:en_us+gameLocale:en_us+channel:+LaunchExp:InApp",
        enc_field(ticket),
        enc_field(tracker_id)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = "123456789";
    const J: &str = "11111111-2222-3333-4444-555555555555";
    const BASE: &str = "https://www.roblox.com/Game/PlaceLauncher.ashx?";
    const TAIL: &str = "&isPlayTogetherGame=false&joinAttemptId=11111111-2222-3333-4444-555555555555&joinAttemptOrigin=PlayButton";

    fn url(req: JoinRequest) -> String {
        build_place_launcher_url(&req, T, J)
    }

    #[test]
    fn place() {
        assert_eq!(
            url(JoinRequest::Place { place_id: 42 }),
            format!("{BASE}request=RequestGame&browserTrackerId=123456789&placeId=42{TAIL}")
        );
    }

    #[test]
    fn job() {
        assert_eq!(
            url(JoinRequest::Job {
                place_id: 42,
                job_id: "abc-def".into()
            }),
            format!("{BASE}request=RequestGameJob&browserTrackerId=123456789&placeId=42&gameId=abc-def{TAIL}")
        );
    }

    #[test]
    fn private_link_code() {
        assert_eq!(
            url(JoinRequest::Private {
                place_id: 42,
                code: PrivateCode::Link("LC".into())
            }),
            format!("{BASE}request=RequestPrivateGame&browserTrackerId=123456789&placeId=42&linkCode=LC{TAIL}")
        );
    }

    #[test]
    fn private_access_code() {
        assert_eq!(
            url(JoinRequest::Private {
                place_id: 42,
                code: PrivateCode::Access("AC".into())
            }),
            format!("{BASE}request=RequestPrivateGame&browserTrackerId=123456789&placeId=42&accessCode=AC{TAIL}")
        );
    }

    #[test]
    fn follow() {
        assert_eq!(
            url(JoinRequest::Follow { user_id: 7 }),
            format!("{BASE}request=RequestFollowUser&browserTrackerId=123456789&userId=7{TAIL}")
        );
    }

    #[test]
    fn param_values_are_encoded() {
        let u = url(JoinRequest::Private {
            place_id: 1,
            code: PrivateCode::Link("a&b=c+d".into()),
        });
        assert!(u.contains("linkCode=a%26b%3Dc%2Bd&"));
    }

    #[test]
    fn uri_shape_and_encoding() {
        let l = url(JoinRequest::Job {
            place_id: 42,
            job_id: "x".into(),
        });
        let u = build_launch_uri("tick+e t/=", 1700000000000, T, Some(&l));
        let pre = "roblox-player:1+launchmode:play+gameinfo:tick%2Be%20t%2F%3D+launchtime:1700000000000+placelauncherurl:";
        let post = "+browsertrackerid:123456789+robloxLocale:en_us+gameLocale:en_us+channel:+LaunchExp:InApp";
        assert!(u.starts_with(pre), "{u}");
        assert!(u.ends_with(post), "{u}");
        let enc = &u[pre.len()..u.len() - post.len()];
        assert!(!enc.contains(['&', '?', '+', '/', ':', '=']), "{enc}");
        assert!(enc.starts_with(
            "https%3A%2F%2Fwww.roblox.com%2FGame%2FPlaceLauncher.ashx%3Frequest%3DRequestGameJob%26"
        ));
    }

    #[test]
    fn home_uri_has_no_place() {
        assert_eq!(
            build_launch_uri("t", 1, T, None),
            "roblox-player:1+launchmode:app+gameinfo:t+launchtime:1+browsertrackerid:123456789+robloxLocale:en_us+gameLocale:en_us+channel:+LaunchExp:InApp"
        );
    }
}

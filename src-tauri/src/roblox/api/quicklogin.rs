//! Roblox Quick Log In (cross-device login): someone signed in to Roblox elsewhere types a
//! 6-character code under Settings > Quick Log In and confirms; this PC then signs in with
//! the code and its private key and gets its own session. `create` and `status` were checked
//! live; `login`'s reply (200 + `Set-Cookie: .ROBLOSECURITY`) is not yet: verify live.
//! The private key never leaves this PC. CSRF handshakes are the client's.

use crate::roblox::{Result, RobloxClient, RobloxError, Secret};
use serde::Deserialize;
use serde_json::{json, Value};
use zeroize::Zeroizing;

const BASE: &str = "/auth-token-service/v1/login";

/// No Debug: holds the private key.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Created {
    pub code: String,
    pub private_key: Zeroizing<String>,
    /// ISO time Roblox drops the code.
    #[serde(default)]
    pub expiration_time: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// "Created" | "UserLinked" | "Validated" | "Cancelled"
    pub status: String,
    #[serde(default)]
    pub account_name: Option<String>,
}

/// POST with a body holding the private key; the body's strings are wiped after.
async fn post_wiped<T: serde::de::DeserializeOwned>(
    c: &RobloxClient,
    path: &str,
    mut body: Value,
) -> Result<T> {
    let r = c.post_json("apis", path, &body, None).await;
    crate::accounts::wipe_value(&mut body);
    r
}

/// New code + private key. No auth needed. The key is masked in logs from here on; the
/// 6-character code is shown on screen anyway, and masking it would garble log lines.
pub async fn create(c: &RobloxClient) -> Result<Created> {
    let n: Created = c
        .post_json("apis", &format!("{BASE}/create"), &json!({}), None)
        .await?;
    crate::logging::register_secret(&n.private_key);
    Ok(n)
}

pub async fn status(c: &RobloxClient, code: &str, key: &Secret) -> Result<Status> {
    let body = json!({ "code": code, "privateKey": key.expose() });
    post_wiped(c, &format!("{BASE}/status"), body).await
}

pub async fn cancel(c: &RobloxClient, code: &str) -> Result<()> {
    c.post_json::<Value>(
        "apis",
        &format!("{BASE}/cancel"),
        &json!({ "code": code }),
        None,
    )
    .await
    .map(|_| ())
}

pub const CHALLENGE: &str =
    "Roblox asked for an extra check. Accounts with 2-step verification can not be added this way.";

/// Sign this PC in with an approved code: the new session cookie. Any reply without a
/// valid `.ROBLOSECURITY` is an error; a Roblox challenge (403 + `rblx-challenge-*`) is
/// `Message(CHALLENGE)` (only its type is logged).
pub async fn login(c: &RobloxClient, code: &str, key: &Secret) -> Result<Secret> {
    let mut body = json!({ "ctype": "AuthToken", "cvalue": code, "password": key.expose() });
    let r = c.post_for_session("auth", "/v2/login", &body).await;
    crate::accounts::wipe_value(&mut body);
    let r = r.map_err(|e| match e {
        RobloxError::Challenge(t) => {
            log::warn!("Quick Log In: Roblox challenge {t:?}");
            RobloxError::Message(CHALLENGE.into())
        }
        e => e,
    });
    r?.and_then(|s| crate::accounts::normalize_cookie(s.expose()))
        .map(Secret::new)
        .ok_or_else(|| RobloxError::Message("Roblox did not sign this computer in.".into()))
}

/// Best effort: end the session `cookie` on Roblox (CSRF handshake is the client's).
pub async fn logout(c: &RobloxClient, cookie: &Secret) -> Result<()> {
    c.post_json::<Value>("auth", "/v2/logout", &json!({}), Some(cookie))
        .await
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        http::{HeaderMap, StatusCode},
        response::IntoResponse,
        routing::post,
        Json, Router,
    };
    use std::sync::{Arc, Mutex};

    const COOKIE: &str = "_|WARNING:-DO-NOT-SHARE-THIS.--Sharing-this-will-allow-someone-to-log-in-as-you-and-to-steal-your-ROBUX-and-items.|_TESTCOOKIE";

    type Seen = Arc<Mutex<Vec<(String, Value)>>>;

    /// Fake auth-token-service + v2/login: every POST needs x-csrf-token "tok" first.
    fn app(seen: Seen, set_cookie: Option<&'static str>) -> Router {
        let h = move |path: &'static str, reply: Value| {
            let seen = seen.clone();
            post(move |hd: HeaderMap, Json(b): Json<Value>| async move {
                if !hd.get("x-csrf-token").is_some_and(|v| v == "tok") {
                    return (StatusCode::FORBIDDEN, [("x-csrf-token", "tok")]).into_response();
                }
                seen.lock().unwrap().push((path.to_owned(), b));
                if path == "login" {
                    let mut r = Json(json!({ "user": { "id": 7 } })).into_response();
                    if let Some(c) = set_cookie {
                        r.headers_mut().append("set-cookie", c.parse().unwrap());
                    }
                    return r;
                }
                Json(reply).into_response()
            })
        };
        Router::new()
            .route(
                "/auth-token-service/v1/login/create",
                h("create", json!({ "code": "ABC123", "status": "Created", "privateKey": "pk-1", "expirationTime": "2030-01-01T00:00:00.000Z", "imagePath": "/x" })),
            )
            .route(
                "/auth-token-service/v1/login/status",
                h("status", json!({ "status": "Validated", "accountName": "Kid", "expirationTime": "x" })),
            )
            .route("/auth-token-service/v1/login/cancel", h("cancel", Value::Null))
            .route("/v2/login", h("login", Value::Null))
    }

    #[tokio::test]
    async fn create_status_cancel_login() {
        let seen: Seen = Arc::default();
        let set = Box::leak(
            format!(".ROBLOSECURITY={COOKIE}; domain=.roblox.com; HttpOnly").into_boxed_str(),
        );
        let c = crate::roblox::mock(app(seen.clone(), Some(set))).await;
        let n = create(&c).await.unwrap();
        assert_eq!(
            (n.code.as_str(), n.private_key.as_str()),
            ("ABC123", "pk-1")
        );
        assert_eq!(
            n.expiration_time.as_deref(),
            Some("2030-01-01T00:00:00.000Z")
        );
        assert_eq!(crate::logging::redact("key pk-1"), "key [REDACTED]");
        let key = Secret::new("pk-1");
        let s = status(&c, "ABC123", &key).await.unwrap();
        assert_eq!(
            (s.status.as_str(), s.account_name.as_deref()),
            ("Validated", Some("Kid"))
        );
        cancel(&c, "ABC123").await.unwrap();
        assert_eq!(login(&c, "ABC123", &key).await.unwrap().expose(), COOKIE);
        let s = seen.lock().unwrap();
        assert_eq!(
            s[1],
            (
                "status".into(),
                json!({ "code": "ABC123", "privateKey": "pk-1" })
            )
        );
        assert_eq!(s[2], ("cancel".into(), json!({ "code": "ABC123" })));
        assert_eq!(
            s[3],
            (
                "login".into(),
                json!({ "ctype": "AuthToken", "cvalue": "ABC123", "password": "pk-1" })
            )
        );
    }

    #[tokio::test]
    async fn login_without_a_session_cookie_is_an_error() {
        for set in [
            None,
            Some(".ROBLOSECURITY=; expires=Thu, 01 Jan 1970 00:00:00 GMT"),
            Some(".ROBLOSECURITY=short"),
        ] {
            let c = crate::roblox::mock(app(Arc::default(), set)).await;
            let e = login(&c, "ABC123", &Secret::new("pk")).await.unwrap_err();
            assert!(matches!(e, RobloxError::Message(_)), "{set:?}");
        }
    }

    #[tokio::test]
    async fn login_redirect_is_an_error_and_body_not_resent() {
        let hits = Arc::new(Mutex::new(0));
        let h2 = hits.clone();
        let app = Router::new()
            .route(
                "/v2/login",
                post(|| async {
                    (
                        StatusCode::TEMPORARY_REDIRECT,
                        [("location", "/v2/elsewhere")],
                    )
                }),
            )
            .route(
                "/v2/elsewhere",
                post(move || async move {
                    *h2.lock().unwrap() += 1;
                    StatusCode::OK
                }),
            );
        let c = crate::roblox::mock(app).await;
        let e = login(&c, "ABC123", &Secret::new("pk")).await.unwrap_err();
        assert!(matches!(e, RobloxError::Status { code: 307, .. }), "{e:?}");
        assert_eq!(*hits.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn login_takes_session_cookie_after_other_cookies() {
        let app = Router::new().route(
            "/v2/login",
            post(|| async {
                let mut r = Json(json!({})).into_response();
                let h = r.headers_mut();
                h.append("set-cookie", "RBXEventTrackerV2=x; path=/".parse().unwrap());
                h.append("set-cookie", "rbxas=y; path=/".parse().unwrap());
                let c = format!(".ROBLOSECURITY={COOKIE}; HttpOnly");
                h.append("set-cookie", c.parse().unwrap());
                r
            }),
        );
        let c = crate::roblox::mock(app).await;
        let s = login(&c, "ABC123", &Secret::new("pk")).await.unwrap();
        assert_eq!(s.expose(), COOKIE);
    }

    #[tokio::test]
    async fn login_challenge_is_its_own_message() {
        let app = Router::new().route(
            "/v2/login",
            post(|| async {
                (
                    StatusCode::FORBIDDEN,
                    [
                        ("rblx-challenge-id", "cid"),
                        ("rblx-challenge-type", "twostepverification"),
                    ],
                    Json(json!({ "errors": [{ "code": 0, "message": "Challenge is required" }] })),
                )
            }),
        );
        let c = crate::roblox::mock(app).await;
        let e = login(&c, "ABC123", &Secret::new("pk")).await.unwrap_err();
        assert!(
            matches!(&e, RobloxError::Message(m) if m == CHALLENGE),
            "{e:?}"
        );
    }

    #[tokio::test]
    async fn logout_posts_with_cookie() {
        let seen: Seen = Arc::default();
        let s2 = seen.clone();
        let app = Router::new().route(
            "/v2/logout",
            post(move |hd: HeaderMap| async move {
                let c = hd.get("cookie").map(|v| v.to_str().unwrap().to_owned());
                s2.lock().unwrap().push((
                    "logout".into(),
                    json!(c.is_some_and(|c| c.contains(COOKIE))),
                ));
                StatusCode::OK
            }),
        );
        let c = crate::roblox::mock(app).await;
        logout(&c, &Secret::new(COOKIE)).await.unwrap();
        assert_eq!(*seen.lock().unwrap(), vec![("logout".into(), json!(true))]);
    }

    #[tokio::test]
    async fn login_refused_is_status() {
        let app = Router::new().route("/v2/login", post(|| async { StatusCode::BAD_REQUEST }));
        let c = crate::roblox::mock(app).await;
        let e = login(&c, "ABC123", &Secret::new("pk")).await.unwrap_err();
        assert!(matches!(e, RobloxError::Status { code: 400, .. }));
    }
}

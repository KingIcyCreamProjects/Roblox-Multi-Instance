//! Local HTTP API (spec §9.3). Bearer token on every request; each family has a toggle.

use super::protocol::ApiError;
use super::{Ctx, LaunchReq, Patch};
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Request, State};
use axum::http::header::{AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, TRANSFER_ENCODING};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;

type Res = Result<Response, ApiError>;

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(self.body())).into_response()
    }
}

fn ok(v: impl Serialize) -> Res {
    Ok(Json(v).into_response())
}

fn body<T: DeserializeOwned>(b: Result<Json<T>, JsonRejection>) -> Result<T, ApiError> {
    b.map(|Json(v)| v).map_err(|e| match e.status().as_u16() {
        413 => ApiError::new(413, "too_large", "Request body is over 2 MB"),
        _ => ApiError::new(400, "bad_request", e.body_text()),
    })
}

impl Ctx {
    /// `family` is the settings suffix: Read, Launch, Edit, Social, Secrets.
    fn gate(&self, family: &str) -> Result<(), ApiError> {
        if self.flag(&format!("/api/enable{family}")) {
            Ok(())
        } else {
            Err(ApiError::new(
                403,
                "family_disabled",
                format!("The {family} API family is turned off in Settings > Automation"),
            ))
        }
    }
}

pub fn router(ctx: Ctx) -> Router {
    Router::new()
        .route("/v1/accounts", get(list))
        .route("/v1/accounts/import-cookie", post(import_cookie))
        .route("/v1/accounts/{name}", get(get_one).patch(patch))
        .route("/v1/accounts/{name}/presence", get(presence))
        .route("/v1/accounts/{name}/launch", post(launch))
        .route("/v1/launches/{id}", get(launch_status))
        .route("/v1/accounts/{name}/block", post(block))
        .route("/v1/accounts/{name}/unblock", post(unblock))
        .route("/v1/accounts/{name}/blocked", get(blocked))
        .route("/v1/accounts/{name}/cookie", get(cookie))
        .fallback(|| async { ApiError::not_found("No such endpoint") })
        .method_not_allowed_fallback(|| async {
            ApiError::new(405, "method_not_allowed", "Wrong method for this endpoint")
        })
        // ponytail: per-request timeout + in-flight cap; idle/slow-header sockets are not
        // capped (axum::serve has no header-read timeout). Loopback-only by default.
        .layer(
            tower::ServiceBuilder::new()
                .layer(axum::error_handling::HandleErrorLayer::new(|_| async {
                    // A vault write already handed to a blocking thread still finishes.
                    ApiError::new(
                        408,
                        "timeout",
                        "The request took too long; a change it made may still apply",
                    )
                }))
                .timeout(std::time::Duration::from_secs(30))
                // Global: Router::layer wraps each route, so share one semaphore.
                .layer(tower::limit::GlobalConcurrencyLimitLayer::new(64)),
        )
        .layer(middleware::from_fn_with_state(ctx.clone(), auth))
        .layer(middleware::from_fn_with_state(ctx.clone(), super::guard))
        .with_state(ctx)
}

async fn auth(State(ctx): State<Ctx>, req: Request, next: Next) -> Response {
    let ok = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .is_some_and(|(_, t)| ctx.token_ok(t.trim()));
    if !ok {
        return ApiError::new(401, "unauthorized", "Missing or wrong bearer token").into_response();
    }
    next.run(req).await
}

async fn list(State(c): State<Ctx>) -> Res {
    c.gate("Read")?;
    ok(c.backend.list_accounts().await?)
}

async fn get_one(State(c): State<Ctx>, Path(name): Path<String>) -> Res {
    c.gate("Read")?;
    ok(c.backend.get_account(&name).await?)
}

async fn presence(State(c): State<Ctx>, Path(name): Path<String>) -> Res {
    c.gate("Read")?;
    ok(c.backend.presence(&name).await?)
}

async fn launch(
    State(c): State<Ctx>,
    Path(name): Path<String>,
    headers: HeaderMap,
    b: Result<Json<LaunchReq>, JsonRejection>,
) -> Res {
    c.gate("Launch")?;
    // No body (so no Content-Type) = all fields default: the account's saved target. A body
    // with a wrong or missing Content-Type is a 400, not silently the saved target.
    let empty = !headers.contains_key(CONTENT_TYPE)
        && !headers.contains_key(TRANSFER_ENCODING)
        && headers.get(CONTENT_LENGTH).is_none_or(|v| v == "0");
    let req = match b {
        Err(JsonRejection::MissingJsonContentType(_)) if empty => LaunchReq::default(),
        b => body(b)?,
    };
    let id = c.backend.launch(&name, req).await?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "launchId": id }))).into_response())
}

async fn launch_status(State(c): State<Ctx>, Path(id): Path<String>) -> Res {
    c.gate("Launch")?;
    ok(c.backend.launch_status(&id).await?)
}

async fn patch(
    State(c): State<Ctx>,
    Path(name): Path<String>,
    b: Result<Json<Patch>, JsonRejection>,
) -> Res {
    c.gate("Edit")?;
    ok(c.backend.patch_account(&name, body(b)?).await?)
}

#[derive(Deserialize)]
struct CookieBody {
    cookie: String,
}

async fn import_cookie(State(c): State<Ctx>, b: Result<Json<CookieBody>, JsonRejection>) -> Res {
    c.gate("Edit")?;
    ok(c.backend.import_cookie(&body(b)?.cookie).await?)
}

#[derive(Deserialize)]
struct UserBody {
    username: String,
}

async fn block(
    State(c): State<Ctx>,
    Path(name): Path<String>,
    b: Result<Json<UserBody>, JsonRejection>,
) -> Res {
    c.gate("Social")?;
    c.backend.block(&name, &body(b)?.username).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn unblock(
    State(c): State<Ctx>,
    Path(name): Path<String>,
    b: Result<Json<UserBody>, JsonRejection>,
) -> Res {
    c.gate("Social")?;
    c.backend.unblock(&name, &body(b)?.username).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn blocked(State(c): State<Ctx>, Path(name): Path<String>) -> Res {
    c.gate("Social")?;
    ok(c.backend.blocked(&name).await?)
}

async fn cookie(State(c): State<Ctx>, Path(name): Path<String>, req: Request) -> Res {
    c.gate("Secrets")?;
    // Plain HTTP: even with LAN on, a cookie never crosses the network. No peer info =
    // in-process (tests); the real server always records it.
    let peer = req
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>();
    if peer.is_some_and(|p| !p.0.ip().is_loopback()) {
        return Err(ApiError::new(
            403,
            "loopback_only",
            "Secrets are only served to this PC (127.0.0.1)",
        ));
    }
    // Every call is logged (never the value); the UI toasts once the account was found.
    log::warn!("automation API: cookie requested for account {name:?}");
    let cookie = c.backend.cookie(&name).await?;
    c.hub.notify_secret(&name);
    ok(json!({ "cookie": cookie }))
}

#[cfg(test)]
mod tests {
    use super::super::{testkit, Automation};
    use super::*;
    use axum::body::Body;
    use serde_json::Value;
    use tower::ServiceExt;

    const ALL: [&str; 5] = ["Read", "Launch", "Edit", "Social", "Secrets"];

    /// (method, path, body, family)
    const ROUTES: &[(&str, &str, &str, &str)] = &[
        ("GET", "/v1/accounts", "", "Read"),
        ("GET", "/v1/accounts/alice", "", "Read"),
        ("GET", "/v1/accounts/alice/presence", "", "Read"),
        (
            "POST",
            "/v1/accounts/alice/launch",
            r#"{"placeId":1}"#,
            "Launch",
        ),
        ("GET", "/v1/launches/L1", "", "Launch"),
        ("PATCH", "/v1/accounts/alice", r#"{"alias":"A"}"#, "Edit"),
        (
            "POST",
            "/v1/accounts/import-cookie",
            r#"{"cookie":"x"}"#,
            "Edit",
        ),
        (
            "POST",
            "/v1/accounts/alice/block",
            r#"{"username":"z"}"#,
            "Social",
        ),
        (
            "POST",
            "/v1/accounts/alice/unblock",
            r#"{"username":"z"}"#,
            "Social",
        ),
        ("GET", "/v1/accounts/alice/blocked", "", "Social"),
        ("GET", "/v1/accounts/alice/cookie", "", "Secrets"),
    ];

    fn with(families: &[&str]) -> Automation {
        let mut api = serde_json::Map::new();
        for f in families {
            api.insert(format!("enable{f}"), Value::Bool(true));
        }
        testkit::automation(Value::Object(api))
    }

    async fn call(
        a: &Automation,
        method: &str,
        path: &str,
        b: &str,
        auth: Option<String>,
    ) -> (u16, Value) {
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header("host", "127.0.0.1:7811")
            .header("content-type", "application/json");
        if let Some(t) = auth {
            req = req.header("authorization", t);
        }
        let res = router(a.ctx.clone())
            .oneshot(req.body(Body::from(b.to_string())).unwrap())
            .await
            .unwrap();
        let status = res.status().as_u16();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    fn bearer(a: &Automation) -> Option<String> {
        Some(format!("Bearer {}", a.token()))
    }

    #[tokio::test]
    async fn missing_or_wrong_token_is_401() {
        let a = with(&ALL);
        for auth in [
            None,
            Some("Bearer nope".into()),
            Some(a.token()),
            Some(format!("Basic {}", a.token())),
        ] {
            let (s, b) = call(&a, "GET", "/v1/accounts", "", auth).await;
            assert_eq!(s, 401);
            assert_eq!(b["error"]["code"], "unauthorized");
        }
    }

    #[tokio::test]
    async fn wrong_method_405_and_big_body_413_use_the_error_shape() {
        let a = with(&ALL);
        let (s, b) = call(&a, "DELETE", "/v1/accounts", "", bearer(&a)).await;
        assert_eq!(
            (s, &b["error"]["code"]),
            (405, &json!("method_not_allowed"))
        );
        let big = format!(r#"{{"cookie":"{}"}}"#, "x".repeat(3 << 20));
        let (s, b) = call(&a, "POST", "/v1/accounts/import-cookie", &big, bearer(&a)).await;
        assert_eq!((s, &b["error"]["code"]), (413, &json!("too_large")));
    }

    #[tokio::test]
    async fn every_family_off_is_403() {
        let a = with(&[]);
        for (m, p, b, f) in ROUTES {
            let (s, body) = call(&a, m, p, b, bearer(&a)).await;
            assert_eq!(s, 403, "{m} {p} ({f})");
            assert_eq!(body["error"]["code"], "family_disabled");
        }
    }

    #[tokio::test]
    async fn only_its_own_family_unlocks_a_route() {
        for fam in ALL {
            let a = with(&[fam]);
            for (m, p, b, f) in ROUTES {
                let (s, _) = call(&a, m, p, b, bearer(&a)).await;
                assert_eq!(s == 403, *f != fam, "{fam}: {m} {p} -> {s}");
            }
        }
    }

    #[tokio::test]
    async fn read_lists_camel_case_accounts() {
        let a = with(&["Read"]);
        let (s, b) = call(&a, "GET", "/v1/accounts", "", bearer(&a)).await;
        assert_eq!(s, 200);
        assert_eq!(b[0]["username"], "Alice");
        assert!(b[0].get("displayName").is_some() && b[0].get("cookie").is_none());
        let (s, b) = call(&a, "GET", "/v1/accounts/BOB", "", bearer(&a)).await;
        assert_eq!((s, &b["username"]), (200, &json!("Bob")));
        let (s, b) = call(&a, "GET", "/v1/accounts/nobody", "", bearer(&a)).await;
        assert_eq!((s, &b["error"]["code"]), (404, &json!("not_found")));
    }

    #[tokio::test]
    async fn launch_is_202_with_id() {
        let a = with(&["Launch"]);
        let (s, b) = call(
            &a,
            "POST",
            "/v1/accounts/alice/launch",
            r#"{"placeId":1}"#,
            bearer(&a),
        )
        .await;
        assert_eq!((s, b), (202, json!({ "launchId": "L1" })));
        let (s, b) = call(
            &a,
            "POST",
            "/v1/accounts/alice/launch",
            "not json",
            bearer(&a),
        )
        .await;
        assert_eq!((s, &b["error"]["code"]), (400, &json!("bad_request")));
        // no body, no Content-Type: the saved target (all fields default)
        let req = Request::builder()
            .method("POST")
            .uri("/v1/accounts/alice/launch")
            .header("host", "127.0.0.1:7811")
            .header("authorization", bearer(&a).unwrap())
            .body(Body::empty())
            .unwrap();
        let res = router(a.ctx.clone()).oneshot(req).await.unwrap();
        assert_eq!(res.status().as_u16(), 202);
        // a body with a non-JSON or no Content-Type is refused, never the saved target
        for ct in [Some("application/x-www-form-urlencoded"), None] {
            let mut req = Request::builder()
                .method("POST")
                .uri("/v1/accounts/alice/launch")
                .header("host", "127.0.0.1:7811")
                .header("authorization", bearer(&a).unwrap())
                .header("content-length", "13");
            if let Some(ct) = ct {
                req = req.header("content-type", ct);
            }
            let req = req.body(Body::from(r#"{"placeId":1}"#)).unwrap();
            let res = router(a.ctx.clone()).oneshot(req).await.unwrap();
            assert_eq!(res.status().as_u16(), 400, "{ct:?}");
        }
    }

    #[tokio::test]
    async fn secret_read_emits_hub_event() {
        let a = with(&["Secrets"]);
        let mut ev = a.ctx.hub.take_secret_events().unwrap();
        let (s, b) = call(&a, "GET", "/v1/accounts/alice/cookie", "", bearer(&a)).await;
        assert_eq!((s, &b["cookie"]), (200, &json!("_|WARNING:-fake")));
        assert_eq!(ev.try_recv().unwrap(), "alice");
        // Unknown account: no toast.
        let (s, _) = call(&a, "GET", "/v1/accounts/nobody/cookie", "", bearer(&a)).await;
        assert_eq!(s, 404);
        assert!(ev.try_recv().is_err());
    }

    #[tokio::test]
    async fn secrets_refused_to_lan_peers() {
        let a = with(&["Secrets"]);
        a.ctx.settings.lock().unwrap()["net"]["allowLan"] = json!(true);
        for (ip, want) in [("192.168.1.9", 403), ("127.0.0.1", 200)] {
            let mut req = Request::builder()
                .uri("/v1/accounts/alice/cookie")
                .header("host", "127.0.0.1:7811")
                .header("authorization", bearer(&a).unwrap())
                .body(Body::empty())
                .unwrap();
            let addr: std::net::SocketAddr = format!("{ip}:5000").parse().unwrap();
            req.extensions_mut()
                .insert(axum::extract::ConnectInfo(addr));
            let res = router(a.ctx.clone()).oneshot(req).await.unwrap();
            assert_eq!(res.status().as_u16(), want, "{ip}");
        }
    }

    #[tokio::test]
    async fn bearer_scheme_is_case_insensitive() {
        let a = with(&["Read"]);
        let (s, _) = call(
            &a,
            "GET",
            "/v1/accounts",
            "",
            Some(format!("bEaReR  {}", a.token())),
        )
        .await;
        assert_eq!(s, 200);
    }

    async fn raw(a: &Automation, headers: &[(&str, &str)]) -> (u16, Value) {
        let mut req = Request::builder().uri("/v1/accounts");
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let req = req
            .header("authorization", format!("Bearer {}", a.token()))
            .body(Body::empty())
            .unwrap();
        let res = router(a.ctx.clone()).oneshot(req).await.unwrap();
        let status = res.status().as_u16();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    #[tokio::test]
    async fn non_loopback_host_and_browser_origin_are_403() {
        let a = with(&["Read"]);
        for (h, code) in [
            (vec![("host", "evil.example:7811")], "forbidden_host"),
            (vec![], "forbidden_host"),
            (
                vec![
                    ("host", "localhost:7811"),
                    ("origin", "http://evil.example"),
                ],
                "forbidden_origin",
            ),
        ] {
            let (s, b) = raw(&a, &h).await;
            assert_eq!((s, &b["error"]["code"]), (403, &json!(code)), "{h:?}");
        }
        for h in [
            vec![("host", "[::1]:7811")],
            vec![("host", "LOCALHOST"), ("origin", "null")],
        ] {
            assert_eq!(raw(&a, &h).await.0, 200, "{h:?}");
        }
        // LAN on: any host passes (origin check still applies).
        a.ctx.settings.lock().unwrap()["net"]["allowLan"] = json!(true);
        assert_eq!(raw(&a, &[("host", "192.168.1.5:7811")]).await.0, 200);
    }
}

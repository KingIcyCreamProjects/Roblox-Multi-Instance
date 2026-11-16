use crate::roblox::{Result, RobloxClient, Secret};

/// One-time launch ticket (community-sourced endpoint/headers; verify live). The ticket is
/// in the `rbx-authentication-ticket` response header; CSRF handshake handled by the client.
pub async fn ticket(c: &RobloxClient, cookie: &Secret) -> Result<String> {
    c.post_for_header(
        "auth",
        "/v1/authentication-ticket",
        &[
            ("Referer", "https://www.roblox.com/"),
            ("RBXAuthenticationNegotiation", "1"),
        ],
        "rbx-authentication-ticket",
        cookie,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roblox::{BaseUrls, RobloxError};
    use axum::{http::HeaderMap, http::StatusCode, response::IntoResponse, routing::post, Router};

    async fn serve() -> RobloxClient {
        let app = Router::new().route(
            "/v1/authentication-ticket",
            post(|h: HeaderMap| async move {
                let get = |k: &str| h.get(k).and_then(|v| v.to_str().ok()).unwrap_or("");
                if !get("cookie").contains("good") {
                    return StatusCode::UNAUTHORIZED.into_response();
                }
                if get("x-csrf-token") != "tok" {
                    return (StatusCode::FORBIDDEN, [("x-csrf-token", "tok")]).into_response();
                }
                if get("referer") != "https://www.roblox.com/"
                    || get("rbxauthenticationnegotiation") != "1"
                {
                    return StatusCode::BAD_REQUEST.into_response();
                }
                (StatusCode::OK, [("rbx-authentication-ticket", "TICKET123")]).into_response()
            }),
        );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        RobloxClient::new(BaseUrls::all(&url))
    }

    #[tokio::test]
    async fn ticket_after_csrf_handshake() {
        let c = serve().await;
        assert_eq!(ticket(&c, &Secret::new("good")).await.unwrap(), "TICKET123");
    }

    #[tokio::test]
    async fn ticket_bad_cookie_is_unauthorized() {
        let c = serve().await;
        let e = ticket(&c, &Secret::new("bad")).await.unwrap_err();
        assert!(matches!(e, RobloxError::Unauthorized), "{e:?}");
    }

    #[tokio::test]
    async fn session_check_reads_user_or_unauthorized() {
        use axum::{routing::get, Json};
        let app = Router::new().route(
            "/v1/users/authenticated",
            get(|h: HeaderMap| async move {
                let good = h
                    .get("cookie")
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|v| v.contains("good"));
                if good {
                    Json(serde_json::json!({"id": 7, "name": "bob", "displayName": "Bob"}))
                        .into_response()
                } else {
                    StatusCode::UNAUTHORIZED.into_response()
                }
            }),
        );
        let c = crate::roblox::mock(app).await;
        let u = crate::roblox::api::users::authenticated(&c, &Secret::new("good"))
            .await
            .unwrap();
        assert_eq!((u.id, u.display_name.as_str()), (7, "Bob"));
        assert!(matches!(
            crate::roblox::api::users::authenticated(&c, &Secret::new("bad")).await,
            Err(RobloxError::Unauthorized)
        ));
    }
}

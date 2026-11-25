pub mod account;
pub mod auth;
pub mod avatar;
pub mod economy;
pub mod friends;
pub mod gamejoin;
pub mod games;
pub mod presence;
pub mod quicklogin;
pub mod servers;
pub mod thumbnails;
pub mod users;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roblox::{BaseUrls, RobloxClient, RobloxError, Secret};
    use axum::{
        extract::State,
        http::{HeaderMap, StatusCode},
        response::IntoResponse,
        routing::{get, patch, post},
        Json, Router,
    };
    use serde_json::{json, Value};
    use std::sync::{Arc, Mutex};

    /// Every request body seen, keyed by path.
    #[derive(Default)]
    struct Mock {
        bodies: Mutex<Vec<(String, Value)>>,
    }

    fn csrf_ok(h: &HeaderMap) -> bool {
        h.get("x-csrf-token").is_some_and(|v| v == "tok")
    }

    fn need_csrf() -> axum::response::Response {
        (StatusCode::FORBIDDEN, [("x-csrf-token", "tok")]).into_response()
    }

    async fn serve() -> (RobloxClient, Arc<Mock>) {
        let m = Arc::new(Mock::default());
        let rec = |path: &'static str, reply: Value| {
            move |State(m): State<Arc<Mock>>, h: HeaderMap, Json(b): Json<Value>| async move {
                if !csrf_ok(&h) {
                    return need_csrf();
                }
                m.bodies.lock().unwrap().push((path.into(), b));
                Json(reply).into_response()
            }
        };
        let app = Router::new()
            .route(
                "/v1/users/7/display-names",
                patch(rec("display", json!({}))),
            )
            .route(
                "/v1/logoutfromallsessionsandreauthenticate",
                post(|h: HeaderMap| async move {
                    if !csrf_ok(&h) {
                        return need_csrf();
                    }
                    (
                        StatusCode::OK,
                        [
                            ("set-cookie", "RBXEventTrackerV2=x; path=/"),
                            // deletion of the old one comes first
                            (
                                "set-cookie",
                                ".ROBLOSECURITY=; expires=Thu, 01 Jan 1970 00:00:00 GMT",
                            ),
                            (
                                "set-cookie",
                                ".ROBLOSECURITY=NEWCOOKIE; domain=.roblox.com; HttpOnly",
                            ),
                        ],
                        // a body that is not JSON must not lose the issued cookie
                        "<html>",
                    )
                        .into_response()
                }),
            )
            .route(
                "/v2/user/passwords/change",
                post(
                    |State(m): State<Arc<Mock>>, h: HeaderMap, Json(b): Json<Value>| async move {
                        if !csrf_ok(&h) {
                            return need_csrf();
                        }
                        m.bodies.lock().unwrap().push(("pw".into(), b));
                        (
                            StatusCode::OK,
                            [("set-cookie", ".ROBLOSECURITY=ROTATED; HttpOnly")],
                            "{}",
                        )
                            .into_response()
                    },
                ),
            )
            .route(
                "/v1/email",
                post(|h: HeaderMap| async move {
                    if !csrf_ok(&h) {
                        return need_csrf();
                    }
                    // PIN locked: csrf passed, still 403
                    (
                        StatusCode::FORBIDDEN,
                        r#"{"errors":[{"code":0,"message":"Account PIN is locked. Unlock it first."}]}"#,
                    )
                        .into_response()
                })
                .get(|| async { Json(json!({"emailAddress": "a***@x.com", "verified": true})) }),
            )
            .route(
                "/v1/account/pin",
                get(|| async { Json(json!({"isEnabled": true, "unlockedUntil": null})) }),
            )
            .route(
                "/v1/account/pin/unlock",
                post(rec("pin", json!({"isEnabled": true, "unlockedUntil": 300.0}))),
            )
            .route(
                "/v1/users/7/currency",
                get(|| async { Json(json!({"robux": 42})) }),
            )
            .route(
                "/v1/users/9/request-friendship",
                post(rec(
                    "friend",
                    json!({"success": false, "isCaptchaRequired": true}),
                )),
            )
            .route(
                "/user-blocking-api/v1/users/9/block-user",
                post(rec("block", json!(null))),
            )
            .route(
                "/user-blocking-api/v1/users/get-blocked-users",
                get(|| async { Json(json!({"data": {"blockedUserIds": [3, 4], "cursor": null}})) }),
            )
            .route(
                "/v1/users",
                post(|Json(b): Json<Value>| async move {
                    let d: Vec<_> = b["userIds"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|i| json!({"id": i, "name": format!("u{i}"), "displayName": format!("U{i}")}))
                        .collect();
                    Json(json!({"data": d}))
                }),
            )
            .route(
                "/v1/users/7",
                get(|| async {
                    Json(json!({"id": 7, "name": "bob", "displayName": "Bob", "created": "2020-01-02T03:04:05.6Z"}))
                }),
            )
            .route(
                "/v1/outfits/55/details",
                get(|| async {
                    Json(json!({
                        "id": 55, "name": "Fit",
                        "assets": [{"id": 1, "name": "Shirt"}, {"id": 2, "name": "Hat"}],
                        "bodyColors": {"headColorId": 226, "torsoColorId": 1017},
                        "scale": {"height": 1.0, "width": 1.0, "head": 1.0, "depth": 1.0, "proportion": 0.0, "bodyType": 0.0},
                        "playerAvatarType": "R6"
                    }))
                }),
            )
            .route(
                "/v1/outfits/66/details",
                get(|| async {
                    Json(json!({"id": 66, "assets": [], "bodyColors": null, "playerAvatarType": "R15"}))
                }),
            )
            .route("/v1/avatar/set-body-colors", post(rec("colors", json!({"success": true}))))
            .route("/v1/avatar/set-scales", post(rec("scales", json!({"success": true}))))
            .route(
                "/v1/avatar/set-player-avatar-type",
                post(rec("type", json!({"success": true}))),
            )
            .route(
                "/v2/avatar/set-wearing-assets",
                post(rec(
                    "wear",
                    json!({"invalidAssets": [], "invalidAssetIds": [2], "success": true}),
                )),
            )
            .route(
                "/v2/avatar/users/7/outfits",
                get(|q: axum::extract::RawQuery| async move {
                    if q.0.unwrap_or_default().contains("paginationToken=P%2B2") {
                        Json(json!({"data": [{"id": 56, "name": "Two"}], "paginationToken": ""}))
                    } else {
                        Json(json!({"data": [{"id": 55, "name": "Fit", "isEditable": true}], "paginationToken": "P+2"}))
                    }
                }),
            )
            .route(
                "/v1/users/outfits",
                get(|| async {
                    Json(json!({"data": [{"targetId": 55, "state": "Completed", "imageUrl": "https://tr.rbxcdn.com/o"}]}))
                }),
            )
            .with_state(m.clone());
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        (RobloxClient::new(BaseUrls::all(&url)), m)
    }

    fn ck() -> Secret {
        Secret::new("good")
    }

    fn body(m: &Mock, path: &str) -> Value {
        m.bodies
            .lock()
            .unwrap()
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, b)| b.clone())
            .unwrap_or_else(|| panic!("no {path} call"))
    }

    #[tokio::test]
    async fn display_name_patch_does_csrf_handshake() {
        let (c, m) = serve().await;
        account::set_display_name(&c, &ck(), 7, "New Name")
            .await
            .unwrap();
        assert_eq!(body(&m, "display"), json!({"newDisplayName": "New Name"}));
    }

    #[tokio::test]
    async fn signout_others_returns_rotated_cookie() {
        let (c, _m) = serve().await;
        let n = account::signout_others(&c, &ck()).await.unwrap().unwrap();
        assert_eq!(n.expose(), "NEWCOOKIE");
    }

    #[tokio::test]
    async fn password_change_sends_fields_and_rotates() {
        let (c, m) = serve().await;
        let n = account::change_password(&c, &ck(), "old", "new")
            .await
            .unwrap();
        assert_eq!(n.unwrap().expose(), "ROTATED");
        assert_eq!(
            body(&m, "pw"),
            json!({"currentPassword": "old", "newPassword": "new"})
        );
    }

    #[tokio::test]
    async fn pin_locked_is_403_with_body() {
        let (c, _m) = serve().await;
        let e = account::set_email(&c, &ck(), "a@b.c", "pw")
            .await
            .unwrap_err();
        assert!(
            matches!(&e, RobloxError::Status { code: 403, body_snippet } if body_snippet.contains("PIN")),
            "{e:?}"
        );
    }

    #[tokio::test]
    async fn reads_decode() {
        let (c, m) = serve().await;
        let e = account::email(&c, &ck()).await.unwrap();
        assert_eq!(
            (e.email_address.as_deref(), e.verified),
            (Some("a***@x.com"), true)
        );
        let p = account::pin_status(&c, &ck()).await.unwrap();
        assert_eq!((p.is_enabled, p.unlocked_until), (true, None));
        let p = account::pin_unlock(&c, &ck(), "1234").await.unwrap();
        assert_eq!(p.unlocked_until, Some(300.0));
        assert_eq!(body(&m, "pin"), json!({"pin": "1234"}));
        assert_eq!(economy::robux(&c, &ck(), 7).await.unwrap(), 42);
        let u = users::user(&c, 7).await.unwrap();
        assert_eq!(
            (u.name.as_str(), u.created.as_str()),
            ("bob", "2020-01-02T03:04:05.6Z")
        );
        let (b, more) = account::blocked_ids(&c, &ck()).await.unwrap();
        assert_eq!((b.as_slice(), more), (&[3, 4][..], false));
        assert!(
            account::page_ids(&json!({})).is_err(),
            "unknown shape is an error"
        );
        assert_eq!(
            account::page_ids(&json!({"data": [{"userId": 5}]})).unwrap(),
            [5]
        );
        let us = users::by_ids(&c, &b).await.unwrap();
        assert_eq!(us[1].display_name, "U4");
    }

    #[test]
    fn outfits_url() {
        assert_eq!(
            avatar::outfits_path(7, ""),
            "/v2/avatar/users/7/outfits?outfitType=Avatar&page=1&itemsPerPage=50"
        );
        assert_eq!(
            avatar::outfits_path(7, "a+b"),
            "/v2/avatar/users/7/outfits?outfitType=Avatar&page=1&itemsPerPage=50&paginationToken=a%2Bb"
        );
    }

    #[test]
    fn cursor_encoding() {
        assert_eq!(account::encode("a+b/c=="), "a%2Bb%2Fc%3D%3D");
        assert_eq!(account::encode("Ab9-_.~"), "Ab9-_.~");
    }

    #[tokio::test]
    async fn friend_captcha_and_block() {
        let (c, m) = serve().await;
        let r = friends::request(&c, &ck(), 9).await.unwrap();
        assert!(!r.success && r.is_captcha_required);
        account::block(&c, &ck(), 9, true).await.unwrap();
        body(&m, "block");
    }

    #[tokio::test]
    async fn wear_applies_everything_and_lists_unowned() {
        let (c, m) = serve().await;
        let missing = avatar::wear(&c, &ck(), 55).await.unwrap();
        assert_eq!(missing, [(2, "Hat".to_string())]);
        assert_eq!(body(&m, "colors")["headColorId"], 226);
        assert_eq!(body(&m, "scales")["height"], 1.0);
        assert_eq!(body(&m, "type"), json!({"playerAvatarType": "R6"}));
        assert_eq!(body(&m, "wear"), json!({"assets": [{"id": 1}, {"id": 2}]}));
    }

    #[tokio::test]
    async fn wear_skips_missing_parts() {
        let (c, m) = serve().await;
        assert!(avatar::wear(&c, &ck(), 66).await.unwrap().is_empty());
        let paths: Vec<String> = m
            .bodies
            .lock()
            .unwrap()
            .iter()
            .map(|(p, _)| p.clone())
            .collect();
        assert_eq!(paths, ["type", "wear"]);
    }

    #[tokio::test]
    async fn outfits_list_with_thumbs() {
        let (c, _m) = serve().await;
        let o = avatar::outfits(&c, 7).await.unwrap();
        assert_eq!((o[0].id, o[0].name.as_str()), (55, "Fit"));
        assert_eq!(o.len(), 2, "second page via encoded token");
        assert_eq!(o[1].id, 56);
        let t = thumbnails::outfits(&c, &[55]).await.unwrap();
        assert_eq!(t[0].image_url.as_deref(), Some("https://tr.rbxcdn.com/o"));
    }
}

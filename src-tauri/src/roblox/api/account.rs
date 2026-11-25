//! Account settings: PIN, display name, password, email, privacy, sessions, blocks.
//! Endpoints and shapes: docs/ROBLOX-API-NOTES.md (several only checked unauthenticated).
use crate::accounts::wipe_value;
use crate::roblox::{Result, RobloxClient, RobloxError, Secret};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PinStatus {
    pub is_enabled: bool,
    /// Seconds until Roblox locks it again (unverified: may be an epoch time).
    #[serde(default)]
    pub unlocked_until: Option<f64>,
}

pub async fn pin_status(c: &RobloxClient, ck: &Secret) -> Result<PinStatus> {
    c.get_json("auth", "/v1/account/pin", Some(ck)).await
}

pub async fn pin_unlock(c: &RobloxClient, ck: &Secret, pin: &str) -> Result<PinStatus> {
    let mut b = json!({"pin": pin});
    let r = c
        .post_json("auth", "/v1/account/pin/unlock", &b, Some(ck))
        .await;
    wipe_value(&mut b);
    r
}

pub async fn set_display_name(
    c: &RobloxClient,
    ck: &Secret,
    user_id: u64,
    name: &str,
) -> Result<()> {
    c.patch_json::<Value>(
        "users",
        &format!("/v1/users/{user_id}/display-names"),
        &json!({"newDisplayName": name}),
        Some(ck),
    )
    .await
    .map(drop)
}

/// Returns the rotated session cookie if Roblox issued one.
pub async fn change_password(
    c: &RobloxClient,
    ck: &Secret,
    current: &str,
    next: &str,
) -> Result<Option<Secret>> {
    let mut b = json!({"currentPassword": current, "newPassword": next});
    let r = c
        .post_rotating("auth", "/v2/user/passwords/change", &b, ck)
        .await;
    wipe_value(&mut b);
    r
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Email {
    #[serde(default)]
    pub email_address: Option<String>,
    #[serde(default)]
    pub verified: bool,
}

pub async fn email(c: &RobloxClient, ck: &Secret) -> Result<Email> {
    c.get_json("accountsettings", "/v1/email", Some(ck)).await
}

pub async fn set_email(c: &RobloxClient, ck: &Secret, email: &str, password: &str) -> Result<()> {
    let mut b = json!({"password": password, "emailAddress": email});
    let r = c
        .post_json::<Value>("accountsettings", "/v1/email", &b, Some(ck))
        .await;
    wipe_value(&mut b);
    r.map(drop)
}

/// Raw `settings-and-options` (shape unverified; parsed leniently in `tools`).
pub async fn privacy_options(c: &RobloxClient, ck: &Secret) -> Result<Value> {
    c.get_json(
        "apis",
        "/user-settings-api/v1/user-settings/settings-and-options",
        Some(ck),
    )
    .await
}

/// `patch` = `{settingKey: value}`.
pub async fn set_privacy(c: &RobloxClient, ck: &Secret, patch: &Value) -> Result<()> {
    c.post_json::<Value>(
        "apis",
        "/user-settings-api/v1/user-settings",
        patch,
        Some(ck),
    )
    .await
    .map(drop)
}

/// Signs out every other session; returns the new cookie Roblox issues for this one.
pub async fn signout_others(c: &RobloxClient, ck: &Secret) -> Result<Option<Secret>> {
    c.post_rotating(
        "auth",
        "/v1/logoutfromallsessionsandreauthenticate",
        &json!({}),
        ck,
    )
    .await
}

pub async fn block(c: &RobloxClient, ck: &Secret, user_id: u64, on: bool) -> Result<()> {
    let verb = if on { "block-user" } else { "unblock-user" };
    c.post_json::<Value>(
        "apis",
        &format!("/user-blocking-api/v1/users/{user_id}/{verb}"),
        &json!({}),
        Some(ck),
    )
    .await
    .map(drop)
}

pub const BLOCK_PAGES: usize = 20;

/// Blocked user ids over at most `BLOCK_PAGES` pages; `true` = more were left.
pub async fn blocked_ids(c: &RobloxClient, ck: &Secret) -> Result<(Vec<u64>, bool)> {
    let mut out = Vec::new();
    let mut cursor = String::new();
    for _ in 0..BLOCK_PAGES {
        let v: Value = c
            .get_json(
                "apis",
                &format!(
                    "/user-blocking-api/v1/users/get-blocked-users?count=50&cursor={}",
                    encode(&cursor)
                ),
                Some(ck),
            )
            .await?;
        let d = if v["data"].is_object() {
            &v["data"]
        } else {
            &v
        };
        out.extend(page_ids(&v)?);
        match d["cursor"].as_str().or(v["nextPageCursor"].as_str()) {
            Some(n) if !n.is_empty() => cursor = n.to_owned(),
            _ => return Ok((out, false)),
        }
    }
    Ok((out, true))
}

/// Ids on one blocked-users page. An unknown shape is an error, never "nobody blocked".
pub fn page_ids(v: &Value) -> Result<Vec<u64>> {
    let d = if v["data"].is_object() { &v["data"] } else { v };
    // ponytail: shape unverified; accepts ids or {userId}/{id} objects
    let ids = d["blockedUserIds"]
        .as_array()
        .or(d["blockedUsers"].as_array())
        .or(v["data"].as_array())
        .ok_or_else(|| RobloxError::Decode("unexpected blocked-users reply".into()))?;
    Ok(ids
        .iter()
        .filter_map(|x| {
            x.as_u64()
                .or(x["userId"].as_u64())
                .or(x["blockedUserId"].as_u64())
                .or(x["id"].as_u64())
        })
        .collect())
}

/// Percent-encode a query value (RFC 3986 unreserved kept).
pub fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::Query, routing::get, Json, Router};
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn page_ids_accepts_known_shapes() {
        assert_eq!(
            page_ids(&json!({"blockedUsers": [{"blockedUserId": 5}, {"userId": 6}, {"id": 7}, {"nope": 1}]})).unwrap(),
            [5, 6, 7]
        );
        assert_eq!(page_ids(&json!({"data": [3, 4]})).unwrap(), [3, 4]);
        assert_eq!(
            page_ids(&json!({"data": {"blockedUsers": [9]}})).unwrap(),
            [9]
        );
    }

    #[tokio::test]
    async fn blocked_ids_follow_cursor_until_empty() {
        let app = Router::new().route(
            "/user-blocking-api/v1/users/get-blocked-users",
            get(|Query(q): Query<HashMap<String, String>>| async move {
                if q.get("cursor").map(String::as_str) == Some("c1") {
                    Json(json!({"data": {"blockedUserIds": [2], "cursor": ""}}))
                } else {
                    Json(json!({"data": {"blockedUserIds": [1], "cursor": "c1"}}))
                }
            }),
        );
        let c = crate::roblox::mock(app).await;
        let (ids, more) = blocked_ids(&c, &Secret::new("c")).await.unwrap();
        assert_eq!((ids, more), (vec![1, 2], false));
    }

    #[tokio::test]
    async fn blocked_ids_stop_at_page_cap() {
        let app = Router::new().route(
            "/user-blocking-api/v1/users/get-blocked-users",
            get(|| async { Json(json!({"data": {"blockedUserIds": [4], "cursor": "more"}})) }),
        );
        let c = crate::roblox::mock(app).await;
        let (ids, more) = blocked_ids(&c, &Secret::new("c")).await.unwrap();
        assert_eq!((ids.len(), more), (BLOCK_PAGES, true));
    }

    #[tokio::test]
    async fn blocked_ids_follow_top_level_next_page_cursor() {
        let app = Router::new().route(
            "/user-blocking-api/v1/users/get-blocked-users",
            get(|Query(q): Query<HashMap<String, String>>| async move {
                if q.get("cursor").map(String::as_str) == Some("c1") {
                    Json(json!({"blockedUsers": [2]}))
                } else {
                    Json(json!({"blockedUsers": [1], "nextPageCursor": "c1"}))
                }
            }),
        );
        let c = crate::roblox::mock(app).await;
        let (ids, more) = blocked_ids(&c, &Secret::new("c")).await.unwrap();
        assert_eq!((ids, more), (vec![1, 2], false));
    }
}

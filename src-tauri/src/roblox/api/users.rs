use crate::roblox::{Result, RobloxClient, Secret};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AuthUser {
    pub id: u64,
    pub name: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UserRef {
    pub id: u64,
    pub name: String,
    pub display_name: String,
    pub requested_username: String,
}

/// Also the session check: 401 -> `RobloxError::Unauthorized`.
pub async fn authenticated(c: &RobloxClient, cookie: &Secret) -> Result<AuthUser> {
    c.get_json("users", "/v1/users/authenticated", Some(cookie))
        .await
}

/// Unknown names are simply absent from the result.
pub async fn by_usernames(c: &RobloxClient, names: &[String]) -> Result<Vec<UserRef>> {
    #[derive(Deserialize)]
    struct R {
        data: Vec<UserRef>,
    }
    let mut out = Vec::with_capacity(names.len());
    for chunk in names.chunks(100) {
        let r: R = c
            .post_json(
                "users",
                "/v1/usernames/users",
                &json!({"usernames": chunk, "excludeBannedUsers": false}),
                None,
            )
            .await?;
        out.extend(r.data);
    }
    Ok(out)
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UserInfo {
    pub id: u64,
    pub name: String,
    pub display_name: String,
    /// ISO time the account was made.
    pub created: String,
}

pub async fn user(c: &RobloxClient, id: u64) -> Result<UserInfo> {
    c.get_json("users", &format!("/v1/users/{id}"), None).await
}

/// Names for ids (public), batches of 100. Unknown ids are absent.
pub async fn by_ids(c: &RobloxClient, ids: &[u64]) -> Result<Vec<AuthUser>> {
    #[derive(Deserialize)]
    struct R {
        data: Vec<AuthUser>,
    }
    let mut out = Vec::with_capacity(ids.len());
    for chunk in ids.chunks(100) {
        let r: R = c
            .post_json(
                "users",
                "/v1/users",
                &json!({"userIds": chunk, "excludeBannedUsers": false}),
                None,
            )
            .await?;
        out.extend(r.data);
    }
    Ok(out)
}

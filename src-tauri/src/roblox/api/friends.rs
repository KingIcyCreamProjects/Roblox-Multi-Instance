use crate::roblox::{Result, RobloxClient, Secret};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FriendReply {
    #[serde(default)]
    pub success: bool,
    #[serde(default)]
    pub is_captcha_required: bool,
}

pub async fn request(c: &RobloxClient, ck: &Secret, user_id: u64) -> Result<FriendReply> {
    c.post_json(
        "friends",
        &format!("/v1/users/{user_id}/request-friendship"),
        &json!({"friendshipOriginSourceType": "Unknown"}),
        Some(ck),
    )
    .await
}

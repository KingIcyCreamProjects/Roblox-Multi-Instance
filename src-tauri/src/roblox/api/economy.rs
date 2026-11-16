use crate::roblox::{Result, RobloxClient, Secret};
use serde::Deserialize;

pub async fn robux(c: &RobloxClient, ck: &Secret, user_id: u64) -> Result<u64> {
    #[derive(Deserialize)]
    struct R {
        robux: u64,
    }
    let r: R = c
        .get_json(
            "economy",
            &format!("/v1/users/{user_id}/currency"),
            Some(ck),
        )
        .await?;
    Ok(r.robux)
}

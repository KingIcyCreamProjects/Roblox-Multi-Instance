use crate::roblox::{Result, RobloxClient, Secret};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Presence {
    pub user_id: u64,
    /// 0 offline, 1 online, 2 in game, 3 in studio.
    pub user_presence_type: u8,
    #[serde(default)]
    pub last_location: Option<String>,
    #[serde(default)]
    pub place_id: Option<u64>,
    #[serde(default)]
    pub root_place_id: Option<u64>,
    #[serde(default)]
    pub game_id: Option<String>,
    #[serde(default)]
    pub universe_id: Option<u64>,
}

/// POST in chunks of 100. Cookie optional (public presence is limited without one).
pub async fn get(c: &RobloxClient, ids: &[u64], cookie: Option<&Secret>) -> Result<Vec<Presence>> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct R {
        user_presences: Vec<Presence>,
    }
    let mut out = Vec::with_capacity(ids.len());
    for chunk in ids.chunks(100) {
        let r: R = c
            .post_json(
                "presence",
                "/v1/presence/users",
                &json!({"userIds": chunk}),
                cookie,
            )
            .await?;
        out.extend(r.user_presences);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::post, Json, Router};
    use serde_json::Value;
    use std::sync::{
        atomic::{AtomicU32, Ordering::SeqCst},
        Arc,
    };

    #[tokio::test]
    async fn presence_batches_by_hundred_and_defaults_optionals() {
        let calls = Arc::new(AtomicU32::new(0));
        let n = calls.clone();
        let app = Router::new().route(
            "/v1/presence/users",
            post(move |Json(b): Json<Value>| {
                let n = n.clone();
                async move {
                    n.fetch_add(1, SeqCst);
                    let d: Vec<Value> = b["userIds"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|i| json!({"userId": i, "userPresenceType": 0}))
                        .collect();
                    Json(json!({ "userPresences": d }))
                }
            }),
        );
        let c = crate::roblox::mock(app).await;
        let ids: Vec<u64> = (1..=101).collect();
        let p = get(&c, &ids, None).await.unwrap();
        assert_eq!((p.len(), calls.load(SeqCst)), (101, 2));
        assert_eq!(
            p[100],
            Presence {
                user_id: 101,
                user_presence_type: 0,
                last_location: None,
                place_id: None,
                root_place_id: None,
                game_id: None,
                universe_id: None,
            }
        );
    }
}

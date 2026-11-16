use crate::roblox::{Result, RobloxClient, RobloxError, Secret};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Deserialize)]
pub struct OutfitRef {
    pub id: u64,
    pub name: String,
}

pub const MAX_OUTFITS: usize = 500;

/// First page = the live-verified URL; later pages add the token.
pub fn outfits_path(user_id: u64, token: &str) -> String {
    let mut p =
        format!("/v2/avatar/users/{user_id}/outfits?outfitType=Avatar&page=1&itemsPerPage=50");
    if !token.is_empty() {
        p.push_str("&paginationToken=");
        p.push_str(&super::account::encode(token));
    }
    p
}

/// Saved avatar outfits (public), paged by `paginationToken`, at most `MAX_OUTFITS`.
pub async fn outfits(c: &RobloxClient, user_id: u64) -> Result<Vec<OutfitRef>> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct R {
        data: Vec<OutfitRef>,
        #[serde(default)]
        pagination_token: Option<String>,
    }
    let mut out = Vec::new();
    let mut token = String::new();
    while out.len() < MAX_OUTFITS {
        let r: R = c
            .get_json("avatar", &outfits_path(user_id, &token), None)
            .await?;
        let empty = r.data.is_empty();
        out.extend(r.data);
        match r.pagination_token {
            Some(t) if !t.is_empty() && !empty => token = t,
            _ => break,
        }
    }
    out.truncate(MAX_OUTFITS);
    Ok(out)
}

/// Applies an outfit to the cookie's account. Never buys: Roblox skips unowned assets,
/// which come back as `(assetId, name)`. Steps run in order (colors, scales, type,
/// assets) and are not rolled back: a failure part-way leaves the earlier steps applied,
/// and the error then says so.
/// ponytail: no rollback (it would need the old avatar and could fail the same way); add
/// one if half-applied outfits turn out to matter.
pub async fn wear(c: &RobloxClient, ck: &Secret, outfit_id: u64) -> Result<Vec<(u64, String)>> {
    let d: Value = c
        .get_json("avatar", &format!("/v1/outfits/{outfit_id}/details"), None)
        .await?;
    let applied = std::sync::atomic::AtomicBool::new(false);
    let applied = &applied;
    let post = |path: &'static str, body: Value| async move {
        use std::sync::atomic::Ordering::Relaxed;
        match c.post_json::<Value>("avatar", path, &body, Some(ck)).await {
            Ok(v) => {
                applied.store(true, Relaxed);
                Ok(v)
            }
            // a dead session stays Unauthorized so the caller still marks it expired
            Err(e) if applied.load(Relaxed) && !matches!(e, RobloxError::Unauthorized) => {
                Err(RobloxError::Message(format!(
                    "Only part of the outfit was applied before Roblox refused the rest: {e}"
                )))
            }
            Err(e) => Err(e),
        }
    };
    for (path, v) in [
        ("/v1/avatar/set-body-colors", &d["bodyColors"]),
        ("/v1/avatar/set-scales", &d["scale"]),
    ] {
        if v.is_object() {
            post(path, v.clone()).await?;
        }
    }
    if let Some(t) = d["playerAvatarType"].as_str() {
        post(
            "/v1/avatar/set-player-avatar-type",
            json!({"playerAvatarType": t}),
        )
        .await?;
    }
    let assets: Vec<(u64, String)> = d["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| {
            Some((
                a["id"].as_u64()?,
                a["name"].as_str().unwrap_or("").to_owned(),
            ))
        })
        .collect();
    let ids: Vec<Value> = assets.iter().map(|(id, _)| json!({"id": id})).collect();
    let r = post("/v2/avatar/set-wearing-assets", json!({"assets": ids})).await?;
    // ponytail: `invalidAssetIds` unverified; without it nothing can be reported missing
    if !r["invalidAssetIds"].is_array() {
        log::warn!("set-wearing-assets: no invalidAssetIds, missing items unknown");
    }
    let bad: Vec<u64> = r["invalidAssetIds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_u64)
        .collect();
    Ok(assets
        .into_iter()
        .filter(|(id, _)| bad.contains(id))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        http::StatusCode,
        response::IntoResponse,
        routing::{get, post},
        Json, Router,
    };
    use std::sync::{
        atomic::{AtomicU32, Ordering::SeqCst},
        Arc,
    };

    #[tokio::test]
    async fn wear_reports_partial_apply_and_first_step_error() {
        let app = Router::new()
            .route(
                "/v1/outfits/77/details",
                get(|| async {
                    Json(json!({"bodyColors": {"headColorId": 1}, "assets": [{"id": 1, "name": "S"}]}))
                }),
            )
            .route(
                "/v1/outfits/78/details",
                get(|| async { Json(json!({"bodyColors": {"headColorId": 9}, "assets": []})) }),
            )
            .route(
                "/v1/avatar/set-body-colors",
                post(|Json(b): Json<Value>| async move {
                    if b["headColorId"] == 9 {
                        StatusCode::INTERNAL_SERVER_ERROR.into_response()
                    } else {
                        Json(json!({})).into_response()
                    }
                }),
            )
            .route(
                "/v2/avatar/set-wearing-assets",
                post(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
            );
        let c = crate::roblox::mock(app).await;
        let ck = Secret::new("c");
        // colors applied, then the asset step fails: says so
        let e = wear(&c, &ck, 77).await.unwrap_err();
        assert!(
            matches!(&e, RobloxError::Message(m) if m.starts_with("Only part of the outfit was applied")),
            "{e:?}"
        );
        // first step fails, nothing applied yet: the raw error
        let e = wear(&c, &ck, 78).await.unwrap_err();
        assert!(matches!(e, RobloxError::Status { code: 500, .. }), "{e:?}");
    }

    #[tokio::test]
    async fn outfits_stop_on_empty_page_with_token() {
        let hits = Arc::new(AtomicU32::new(0));
        let h = hits.clone();
        let app = Router::new().route(
            "/v2/avatar/users/7/outfits",
            get(move || {
                let h = h.clone();
                async move {
                    let n = h.fetch_add(1, SeqCst);
                    Json(json!({"data": [], "paginationToken": if n == 0 { "x" } else { "" }}))
                }
            }),
        );
        let c = crate::roblox::mock(app).await;
        assert!(outfits(&c, 7).await.unwrap().is_empty());
        assert_eq!(hits.load(SeqCst), 1, "empty page ends the walk");
    }

    #[tokio::test]
    async fn outfits_capped_at_max() {
        let hits = Arc::new(AtomicU32::new(0));
        let h = hits.clone();
        let app = Router::new().route(
            "/v2/avatar/users/7/outfits",
            get(move || {
                let h = h.clone();
                async move {
                    h.fetch_add(1, SeqCst);
                    let data: Vec<Value> = (0..50).map(|i| json!({"id": i, "name": "F"})).collect();
                    Json(json!({"data": data, "paginationToken": "more"}))
                }
            }),
        );
        let c = crate::roblox::mock(app).await;
        assert_eq!(outfits(&c, 7).await.unwrap().len(), MAX_OUTFITS);
        assert_eq!(hits.load(SeqCst), (MAX_OUTFITS / 50) as u32);
    }

    #[tokio::test]
    async fn wear_dead_session_mid_outfit_stays_unauthorized() {
        let app = Router::new()
            .route(
                "/v1/outfits/80/details",
                get(|| async {
                    Json(json!({"bodyColors": {"headColorId": 1}, "scale": {"height": 1.0}, "assets": []}))
                }),
            )
            .route(
                "/v1/avatar/set-body-colors",
                post(|| async { Json(json!({})) }),
            )
            .route(
                "/v1/avatar/set-scales",
                post(|| async { StatusCode::UNAUTHORIZED }),
            );
        let c = crate::roblox::mock(app).await;
        let e = wear(&c, &Secret::new("c"), 80).await.unwrap_err();
        assert!(matches!(e, RobloxError::Unauthorized), "{e:?}");
    }
}

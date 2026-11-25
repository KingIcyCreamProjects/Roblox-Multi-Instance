use crate::roblox::{Result, RobloxClient};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Thumb {
    pub target_id: u64,
    /// "Completed", "Pending", "Blocked", ...; `image_url` is only usable when Completed.
    pub state: String,
    #[serde(default, deserialize_with = "cdn_only")]
    pub image_url: Option<String>,
}

/// Image URLs reach the UI only as https `*.rbxcdn.com` (same check as `get_cdn`).
fn cdn_only<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<String>, D::Error> {
    let s: Option<String> = Deserialize::deserialize(d)?;
    Ok(s.filter(|s| crate::roblox::client::cdn_url(s).is_some()))
}

#[derive(Deserialize)]
struct R {
    data: Vec<Thumb>,
}

/// Image URLs only; disk caching is elsewhere. Roblox caps a batch at 100 ids.
pub async fn headshots(c: &RobloxClient, ids: &[u64], size: &str) -> Result<Vec<Thumb>> {
    let mut out = Vec::with_capacity(ids.len());
    for chunk in ids.chunks(100) {
        let q = chunk
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let r: R = c
            .get_json(
                "thumbnails",
                &format!(
                    "/v1/users/avatar-headshot?userIds={q}&size={size}&format=Png&isCircular=false"
                ),
                None,
            )
            .await?;
        out.extend(r.data);
    }
    Ok(out)
}

pub async fn avatar(c: &RobloxClient, id: u64, size: &str) -> Result<Option<Thumb>> {
    let r: R = c
        .get_json(
            "thumbnails",
            &format!("/v1/users/avatar?userIds={id}&size={size}&format=Png&isCircular=false"),
            None,
        )
        .await?;
    Ok(r.data.into_iter().next())
}

/// Square game icon for one universe.
pub async fn game_icon(c: &RobloxClient, universe_id: u64) -> Result<Option<Thumb>> {
    let r: R = c
        .get_json(
            "thumbnails",
            &format!(
                "/v1/games/icons?universeIds={universe_id}&size=150x150&format=Png&isCircular=false"
            ),
            None,
        )
        .await?;
    Ok(r.data.into_iter().next())
}

/// Headshot URLs for server player tokens (`POST /v1/batch`, 100 per call): token → URL.
/// Only Completed images; the URL equals the userId headshot of the same size.
pub async fn headshots_by_token(
    c: &RobloxClient,
    tokens: &[String],
    size: &str,
) -> Result<HashMap<String, String>> {
    let mut out = HashMap::new();
    for chunk in tokens.chunks(100) {
        let body: Vec<Value> = chunk
            .iter()
            .map(|t| json!({"requestId": t, "token": t, "type": "AvatarHeadShot", "size": size, "format": "Png", "isCircular": false}))
            .collect();
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct B {
            request_id: String,
            state: String,
            #[serde(default, deserialize_with = "cdn_only")]
            image_url: Option<String>,
        }
        #[derive(Deserialize)]
        struct BR {
            data: Vec<B>,
        }
        let r: BR = c
            .post_json("thumbnails", "/v1/batch", &Value::Array(body), None)
            .await?;
        out.extend(
            r.data
                .into_iter()
                .filter(|b| b.state == "Completed")
                .filter_map(|b| Some((b.request_id, b.image_url?))),
        );
    }
    Ok(out)
}

/// Square icons per universe (Completed only).
pub async fn game_icons(c: &RobloxClient, ids: &[u64]) -> Result<HashMap<u64, String>> {
    let mut out = HashMap::new();
    for q in joined(ids) {
        let r: R = c
            .get_json(
                "thumbnails",
                &format!(
                    "/v1/games/icons?universeIds={q}&size=150x150&format=Png&isCircular=false"
                ),
                None,
            )
            .await?;
        out.extend(completed(r.data).map(|t| (t.0, t.1)));
    }
    Ok(out)
}

/// First 16:9 thumbnail (768x432) per universe (Completed only).
pub async fn game_thumbs(c: &RobloxClient, ids: &[u64]) -> Result<HashMap<u64, String>> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct U {
        universe_id: u64,
        #[serde(default)]
        thumbnails: Vec<Thumb>,
    }
    #[derive(Deserialize)]
    struct UR {
        data: Vec<U>,
    }
    let mut out = HashMap::new();
    for q in joined(ids) {
        let r: UR = c
            .get_json(
                "thumbnails",
                &format!("/v1/games/multiget/thumbnails?universeIds={q}&countPerUniverse=1&defaults=true&size=768x432&format=Png&isCircular=false"),
                None,
            )
            .await?;
        for u in r.data {
            if let Some((_, url)) = completed(u.thumbnails).next() {
                out.insert(u.universe_id, url);
            }
        }
    }
    Ok(out)
}

/// Outfit images, batches of 100.
pub async fn outfits(c: &RobloxClient, ids: &[u64]) -> Result<Vec<Thumb>> {
    let mut out = Vec::with_capacity(ids.len());
    for q in joined(ids) {
        let r: R = c
            .get_json(
                "thumbnails",
                &format!(
                    "/v1/users/outfits?userOutfitIds={q}&size=150x150&format=Png&isCircular=false"
                ),
                None,
            )
            .await?;
        out.extend(r.data);
    }
    Ok(out)
}

/// Comma-joined id batches of 100.
fn joined(ids: &[u64]) -> Vec<String> {
    ids.chunks(100)
        .map(|c| c.iter().map(u64::to_string).collect::<Vec<_>>().join(","))
        .collect()
}

fn completed(v: Vec<Thumb>) -> impl Iterator<Item = (u64, String)> {
    v.into_iter()
        .filter(|t| t.state == "Completed")
        .filter_map(|t| Some((t.target_id, t.image_url?)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        extract::Query,
        routing::{get, post},
        Json, Router,
    };
    use serde_json::{json, Value};
    use std::collections::HashMap;

    #[tokio::test]
    async fn token_batch_icons_and_thumbs() {
        let app = Router::new()
            .route(
                "/v1/batch",
                post(|Json(b): Json<Vec<Value>>| async move {
                    assert!(b.len() <= 100);
                    let data: Vec<Value> = b
                        .iter()
                        .map(|r| {
                            assert_eq!((r["type"].as_str(), r["size"].as_str()), (Some("AvatarHeadShot"), Some("150x150")));
                            let t = r["token"].as_str().unwrap();
                            let ok = t != "pending";
                            json!({"requestId": r["requestId"], "targetId": 0, "state": if ok {"Completed"} else {"Pending"},
                                   "imageUrl": if ok { json!(format!("https://tr.rbxcdn.com/{t}")) } else { Value::Null }})
                        })
                        .collect();
                    Json(json!({ "data": data }))
                }),
            )
            .route(
                "/v1/games/icons",
                get(|Query(q): Query<HashMap<String, String>>| async move {
                    assert_eq!(q["universeIds"], "1,2");
                    Json(json!({"data": [{"targetId": 1, "state": "Completed", "imageUrl": "https://tr.rbxcdn.com/i1"},
                                         {"targetId": 2, "state": "Blocked", "imageUrl": null}]}))
                }),
            )
            .route(
                "/v1/games/multiget/thumbnails",
                get(|Query(q): Query<HashMap<String, String>>| async move {
                    assert_eq!((q["universeIds"].as_str(), q["size"].as_str()), ("1,2", "768x432"));
                    Json(json!({"data": [
                        {"universeId": 1, "error": null, "thumbnails": [{"targetId": 9, "state": "Completed", "imageUrl": "https://tr.rbxcdn.com/t1"}]},
                        {"universeId": 2, "error": null, "thumbnails": []}
                    ]}))
                }),
            );
        let c = crate::roblox::mock(app).await;
        let mut toks: Vec<String> = (0..150).map(|i| format!("k{i}")).collect();
        toks.push("pending".into());
        let m = headshots_by_token(&c, &toks, "150x150").await.unwrap();
        assert_eq!(m.len(), 150);
        assert_eq!(m["k149"], "https://tr.rbxcdn.com/k149");
        assert!(!m.contains_key("pending"));
        let i = game_icons(&c, &[1, 2]).await.unwrap();
        assert_eq!(
            (i.get(&1).map(String::as_str), i.get(&2)),
            (Some("https://tr.rbxcdn.com/i1"), None)
        );
        let t = game_thumbs(&c, &[1, 2]).await.unwrap();
        assert_eq!((t[&1].as_str(), t.len()), ("https://tr.rbxcdn.com/t1", 1));
    }

    #[test]
    fn only_rbxcdn_https_urls_kept() {
        let t = |u: &str| {
            serde_json::from_value::<Thumb>(
                json!({"targetId": 1, "state": "Completed", "imageUrl": u}),
            )
            .unwrap()
            .image_url
        };
        assert_eq!(
            t("https://tr.rbxcdn.com/a").as_deref(),
            Some("https://tr.rbxcdn.com/a")
        );
        for bad in [
            "http://tr.rbxcdn.com/a",
            "https://evil.io/x",
            "javascript:alert(1)",
        ] {
            assert_eq!(t(bad), None, "{bad}");
        }
    }

    #[tokio::test]
    async fn headshots_batch_by_hundred() {
        use std::sync::{
            atomic::{AtomicU32, Ordering::SeqCst},
            Arc,
        };
        let calls = Arc::new(AtomicU32::new(0));
        let n = calls.clone();
        let app = Router::new().route(
            "/v1/users/avatar-headshot",
            get(move |Query(q): Query<HashMap<String, String>>| {
                let n = n.clone();
                async move {
                    n.fetch_add(1, SeqCst);
                    let d: Vec<Value> = q["userIds"]
                        .split(',')
                        .map(|i| json!({"targetId": i.parse::<u64>().unwrap(), "state": "Completed", "imageUrl": "u"}))
                        .collect();
                    Json(json!({ "data": d }))
                }
            }),
        );
        let c = crate::roblox::mock(app).await;
        let ids: Vec<u64> = (1..=150).collect();
        let t = headshots(&c, &ids, "150x150").await.unwrap();
        assert_eq!((t.len(), calls.load(SeqCst)), (150, 2));
    }
}

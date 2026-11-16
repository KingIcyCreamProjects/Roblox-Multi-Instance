use crate::roblox::{Result, RobloxClient, Secret};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Game {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub root_place_id: Option<u64>,
    #[serde(default)]
    pub playing: Option<u64>,
    #[serde(default)]
    pub creator: Option<Creator>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Creator {
    pub name: String,
}

/// Universe details (`games.roblox.com/v1/games?universeIds=`), batches of 50.
pub async fn by_universe(c: &RobloxClient, ids: &[u64]) -> Result<Vec<Game>> {
    #[derive(Deserialize)]
    struct R {
        data: Vec<Game>,
    }
    let mut out = Vec::with_capacity(ids.len());
    for chunk in ids.chunks(50) {
        let q = chunk
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let r: R = c
            .get_json("games", &format!("/v1/games?universeIds={q}"), None)
            .await?;
        out.extend(r.data);
    }
    Ok(out)
}

pub async fn universe_of(c: &RobloxClient, place_id: u64) -> Result<u64> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct R {
        universe_id: Option<u64>,
    }
    let r: R = c
        .get_json(
            "apis",
            &format!("/universes/v1/places/{place_id}/universe"),
            None,
        )
        .await?;
    r.universe_id
        .ok_or_else(|| crate::roblox::RobloxError::Decode("place has no universe".into()))
}

/// Share link → raw resolve answer (unverified endpoint, needs a signed-in account).
pub async fn resolve_share(
    c: &RobloxClient,
    code: &str,
    link_type: &str,
    cookie: &Secret,
) -> Result<Value> {
    c.post_json(
        "apis",
        "/sharelinks/v1/resolve-link",
        &json!({ "linkId": code, "linkType": link_type }),
        Some(cookie),
    )
    .await
}

/// One game from omni-search or an explore sort.
#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Hit {
    pub universe_id: u64,
    pub root_place_id: u64,
    pub name: String,
    pub creator_name: Option<String>,
    pub player_count: Option<u64>,
    pub total_up_votes: Option<u64>,
    pub total_down_votes: Option<u64>,
    pub is_sponsored: bool,
}

/// Omni-search (games only): `(hits, nextPageToken)`.
pub async fn search(
    c: &RobloxClient,
    query: &str,
    page_token: Option<&str>,
    session: &str,
) -> Result<(Vec<Hit>, Option<String>)> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Group {
        #[serde(default)]
        content_group_type: String,
        #[serde(default)]
        contents: Vec<Value>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct R {
        #[serde(default)]
        search_results: Vec<Group>,
        next_page_token: Option<String>,
    }
    let e = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
    let mut p = format!(
        "/search-api/omni-search?searchQuery={}&sessionId={}&pageType=all",
        e(query),
        e(session)
    );
    if let Some(t) = page_token {
        p += &format!("&pageToken={}", e(t));
    }
    let r: R = c.get_json("apis", &p, None).await?;
    let hits = r
        .search_results
        .into_iter()
        .filter(|g| g.content_group_type == "Game")
        .flat_map(|g| g.contents)
        .filter_map(|v| serde_json::from_value::<Hit>(v).ok())
        .filter(|h| h.universe_id > 0 && h.root_place_id > 0)
        .collect();
    Ok((hits, r.next_page_token.filter(|t| !t.is_empty())))
}

/// Explore page: "Top Playing Now" (else the first games sort), ads dropped.
pub async fn top_sort(c: &RobloxClient, session: &str) -> Result<Vec<Hit>> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Sort {
        #[serde(default)]
        content_type: String,
        #[serde(default)]
        sort_id: String,
        #[serde(default)]
        games: Vec<Hit>,
    }
    #[derive(Deserialize)]
    struct R {
        #[serde(default)]
        sorts: Vec<Sort>,
    }
    let s: String = url::form_urlencoded::byte_serialize(session.as_bytes()).collect();
    let r: R = c
        .get_json(
            "apis",
            &format!("/explore-api/v1/get-sorts?sessionId={s}"),
            None,
        )
        .await?;
    let mut sorts: Vec<Sort> = r
        .sorts
        .into_iter()
        .filter(|s| s.content_type == "Games")
        .collect();
    let i = sorts
        .iter()
        .position(|s| s.sort_id == "top-playing-now")
        .unwrap_or(0);
    Ok(if sorts.is_empty() {
        vec![]
    } else {
        sorts.swap_remove(i).games
    }
    .into_iter()
    .filter(|h| !h.is_sponsored)
    .collect())
}

/// `(upVotes, downVotes)` per universe.
pub async fn votes(
    c: &RobloxClient,
    ids: &[u64],
) -> Result<std::collections::HashMap<u64, (u64, u64)>> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct V {
        id: u64,
        up_votes: u64,
        down_votes: u64,
    }
    #[derive(Deserialize)]
    struct R {
        data: Vec<V>,
    }
    let mut out = std::collections::HashMap::new();
    for chunk in ids.chunks(50) {
        let q = chunk
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let r: R = c
            .get_json("games", &format!("/v1/games/votes?universeIds={q}"), None)
            .await?;
        out.extend(
            r.data
                .into_iter()
                .map(|v| (v.id, (v.up_votes, v.down_votes))),
        );
    }
    Ok(out)
}

/// Places of a universe `(placeId, name)`, API order, cursor-paged (≤ 10 pages of 100).
pub async fn places(c: &RobloxClient, universe_id: u64) -> Result<Vec<(u64, String)>> {
    #[derive(Deserialize)]
    struct P {
        id: u64,
        name: String,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct R {
        data: Vec<P>,
        next_page_cursor: Option<String>,
    }
    let mut out = vec![];
    let mut cursor: Option<String> = None;
    for _ in 0..10 {
        let mut p = format!("/v1/universes/{universe_id}/places?sortOrder=Asc&limit=100");
        if let Some(cur) = &cursor {
            p += "&cursor=";
            p.extend(url::form_urlencoded::byte_serialize(cur.as_bytes()));
        }
        let r: R = c.get_json("develop", &p, None).await?;
        out.extend(r.data.into_iter().map(|p| (p.id, p.name)));
        match r.next_page_cursor.filter(|n| !n.is_empty()) {
            Some(n) => cursor = Some(n),
            None => break,
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::Query, routing::get, Json, Router};
    use std::collections::HashMap;

    type Q = Query<HashMap<String, String>>;

    fn hit(u: u64, sponsored: bool) -> Value {
        json!({"universeId": u, "rootPlaceId": u * 10, "name": format!("G{u}"), "creatorName": "Maker", "playerCount": 7,
               "totalUpVotes": 3, "totalDownVotes": 1, "isSponsored": sponsored, "contentType": "Game"})
    }

    #[tokio::test]
    async fn search_sorts_votes_places_and_creator() {
        let app = Router::new()
            .route(
                "/search-api/omni-search",
                get(|Query(q): Q| async move {
                    assert_eq!((q["searchQuery"].as_str(), q["sessionId"].as_str()), ("doors & co", "s"));
                    let last = q.get("pageToken").map(String::as_str) == Some("p2");
                    Json(json!({"searchResults": [
                        {"contentGroupType": "Game", "contents": [hit(1, false), hit(2, true)]},
                        {"contentGroupType": "User", "contents": [{"userId": 3}]}
                    ], "nextPageToken": if last { "" } else { "p2" }}))
                }),
            )
            .route(
                "/explore-api/v1/get-sorts",
                get(|| async {
                    Json(json!({"sorts": [
                        {"contentType": "Filters", "sortId": "filters_v5"},
                        {"contentType": "Games", "sortId": "top-trending", "games": [hit(9, false)]},
                        {"contentType": "Games", "sortId": "top-playing-now", "games": [hit(1, false), hit(2, true), hit(3, false)]}
                    ]}))
                }),
            )
            .route(
                "/v1/games/votes",
                get(|Query(q): Q| async move {
                    assert_eq!(q["universeIds"], "1,2");
                    Json(json!({"data": [{"id": 1, "upVotes": 9, "downVotes": 1}, {"id": 2, "upVotes": 0, "downVotes": 0}]}))
                }),
            )
            .route(
                "/v1/universes/4/places",
                get(|Query(q): Q| async move {
                    let second = q.contains_key("cursor");
                    Json(json!({"nextPageCursor": if second { Value::Null } else { json!("c") },
                                "data": [{"id": if second { 41 } else { 40 }, "universeId": 4, "name": if second { "Sub" } else { "Root" }}]}))
                }),
            )
            .route(
                "/v1/games",
                get(|| async {
                    Json(json!({"data": [{"id": 4, "rootPlaceId": 40, "name": "U", "playing": 2, "creator": {"id": 1, "name": "Grp", "type": "Group"}}]}))
                }),
            );
        let c = crate::roblox::mock(app).await;
        let (h, next) = search(&c, "doors & co", None, "s").await.unwrap();
        assert_eq!(
            (
                h.len(),
                h[0].universe_id,
                h[0].root_place_id,
                next.as_deref()
            ),
            (2, 1, 10, Some("p2"))
        );
        assert_eq!(
            (h[0].creator_name.as_deref(), h[0].total_up_votes),
            (Some("Maker"), Some(3))
        );
        let (_, next) = search(&c, "doors & co", Some("p2"), "s").await.unwrap();
        assert_eq!(next, None);
        let top = top_sort(&c, "s").await.unwrap();
        assert_eq!(
            top.iter().map(|h| h.universe_id).collect::<Vec<_>>(),
            vec![1, 3]
        );
        let v = votes(&c, &[1, 2]).await.unwrap();
        assert_eq!((v[&1], v[&2]), ((9, 1), (0, 0)));
        assert_eq!(
            places(&c, 4).await.unwrap(),
            vec![(40, "Root".into()), (41, "Sub".into())]
        );
        let g = by_universe(&c, &[4]).await.unwrap();
        assert_eq!(g[0].creator.as_ref().map(|c| c.name.as_str()), Some("Grp"));
    }

    #[tokio::test]
    async fn search_drops_zero_ids_and_top_sort_falls_back_to_first() {
        let app = Router::new()
            .route(
                "/search-api/omni-search",
                get(|| async {
                    Json(json!({"searchResults": [{"contentGroupType": "Game", "contents": [
                        hit(0, false), {"universeId": 2, "rootPlaceId": 0, "name": "NoRoot"}, hit(3, false)
                    ]}]}))
                }),
            )
            .route(
                "/explore-api/v1/get-sorts",
                get(|| async {
                    Json(json!({"sorts": [
                        {"contentType": "Games", "sortId": "top-trending", "games": [hit(7, false)]},
                        {"contentType": "Games", "sortId": "other", "games": [hit(8, false)]}
                    ]}))
                }),
            );
        let c = crate::roblox::mock(app).await;
        let (h, _) = search(&c, "q", None, "s").await.unwrap();
        assert_eq!(h.iter().map(|h| h.universe_id).collect::<Vec<_>>(), [3]);
        let top = top_sort(&c, "s").await.unwrap();
        assert_eq!(top.iter().map(|h| h.universe_id).collect::<Vec<_>>(), [7]);
    }

    #[tokio::test]
    async fn universe_of_null_is_decode_error() {
        let app = Router::new().route(
            "/universes/v1/places/5/universe",
            get(|| async { Json(json!({"universeId": null})) }),
        );
        let c = crate::roblox::mock(app).await;
        assert!(matches!(
            universe_of(&c, 5).await,
            Err(crate::roblox::RobloxError::Decode(_))
        ));
    }

    #[tokio::test]
    async fn by_universe_batches_of_fifty() {
        use std::sync::{
            atomic::{AtomicU32, Ordering::SeqCst},
            Arc,
        };
        let calls = Arc::new(AtomicU32::new(0));
        let n = calls.clone();
        let app = Router::new().route(
            "/v1/games",
            get(move |Query(q): Q| {
                let n = n.clone();
                async move {
                    n.fetch_add(1, SeqCst);
                    let d: Vec<Value> = q["universeIds"]
                        .split(',')
                        .map(|i| json!({"id": i.parse::<u64>().unwrap(), "name": "G", "rootPlaceId": 1}))
                        .collect();
                    Json(json!({ "data": d }))
                }
            }),
        );
        let c = crate::roblox::mock(app).await;
        let ids: Vec<u64> = (1..=51).collect();
        let g = by_universe(&c, &ids).await.unwrap();
        assert_eq!((g.len(), calls.load(SeqCst)), (51, 2));
    }

    #[tokio::test]
    async fn top_sort_without_games_sorts_is_empty() {
        let app = Router::new().route(
            "/explore-api/v1/get-sorts",
            get(|| async {
                Json(json!({"sorts": [{"contentType": "Filters", "sortId": "filters_v5"}]}))
            }),
        );
        let c = crate::roblox::mock(app).await;
        assert!(top_sort(&c, "s").await.unwrap().is_empty());
    }
}

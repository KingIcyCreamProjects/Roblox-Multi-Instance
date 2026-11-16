//! Game server lists (`games.roblox.com/v1/games/{placeId}/servers/{Public|VIP}`), 100 per page.

use crate::roblox::{Result, RobloxClient, Secret};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Public,
    Private,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Server {
    /// JobId; private servers that aren't running have none.
    pub id: Option<String>,
    pub playing: u32,
    pub max_players: u32,
    pub ping: Option<f64>,
    pub fps: Option<f64>,
    /// Empty without a cookie (docs/ROBLOX-API-NOTES.md).
    pub player_tokens: Vec<String>,
    /// Private server name (VIP list only).
    pub name: Option<String>,
}

/// One page: `(servers, nextPageCursor)`. Private kind lists the cookie owner's private servers.
pub async fn page(
    c: &RobloxClient,
    place_id: u64,
    kind: Kind,
    cursor: Option<&str>,
    cookie: Option<&Secret>,
) -> Result<(Vec<Server>, Option<String>)> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct R {
        data: Vec<Server>,
        next_page_cursor: Option<String>,
    }
    let k = match kind {
        Kind::Public => "Public",
        Kind::Private => "VIP",
    };
    let mut p = format!("/v1/games/{place_id}/servers/{k}?limit=100");
    if let Some(cur) = cursor {
        p += "&cursor=";
        p.extend(url::form_urlencoded::byte_serialize(cur.as_bytes()));
    }
    let r: R = c.get_json("games", &p, cookie).await?;
    Ok((r.data, r.next_page_cursor.filter(|n| !n.is_empty())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::Query, http::HeaderMap, routing::get, Json, Router};
    use serde_json::{json, Value};
    use std::collections::HashMap;

    #[tokio::test]
    async fn parses_public_and_private_pages() {
        let app = Router::new()
            .route(
                "/v1/games/5/servers/Public",
                get(|Query(q): Query<HashMap<String, String>>| async move {
                    assert_eq!(q["limit"], "100");
                    let next = if q.get("cursor").map(String::as_str) == Some("a b") { Value::Null } else { json!("a b") };
                    Json(json!({"previousPageCursor": null, "nextPageCursor": next, "data": [
                        {"id": "j1", "maxPlayers": 10, "playing": 3, "playerTokens": ["t1"], "players": [], "fps": 59.6, "ping": 80},
                        {"id": "j2", "maxPlayers": 10, "playing": 10}
                    ]}))
                }),
            )
            .route(
                "/v1/games/5/servers/VIP",
                get(|h: HeaderMap| async move {
                    assert!(h.get("cookie").is_some());
                    Json(json!({"nextPageCursor": null, "data": [
                        {"id": null, "maxPlayers": 8, "playing": 0, "name": "Squad", "vipServerId": 1},
                        {"id": "p1", "maxPlayers": 8, "playing": 2, "name": "Trade", "fps": null, "ping": null}
                    ]}))
                }),
            );
        let c = crate::roblox::mock(app).await;
        let (s, next) = page(&c, 5, Kind::Public, None, None).await.unwrap();
        assert_eq!(next.as_deref(), Some("a b"));
        assert_eq!(s[0].id.as_deref(), Some("j1"));
        assert_eq!(
            (s[0].playing, s[0].max_players, s[0].ping, s[0].fps),
            (3, 10, Some(80.0), Some(59.6))
        );
        assert_eq!(s[0].player_tokens, vec!["t1"]);
        assert_eq!(
            (s[1].ping, s[1].fps, s[1].player_tokens.len()),
            (None, None, 0)
        );
        let (_, next) = page(&c, 5, Kind::Public, Some("a b"), None).await.unwrap();
        assert_eq!(next, None);
        let ck = Secret::new("c");
        let (p, _) = page(&c, 5, Kind::Private, None, Some(&ck)).await.unwrap();
        assert_eq!(
            (p[0].id.clone(), p[1].name.as_deref()),
            (None, Some("Trade"))
        );
    }
}

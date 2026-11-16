//! `gamejoin.roblox.com/v1/join-game-instance` (unverified live; research-interop.md): asks
//! Roblox to place the account in one server. Used for region lookup (server address) and
//! "set server, then join" (reserve before launching).

use crate::roblox::{Result, RobloxClient, Secret};
use serde_json::Value;

/// Raw answer (`{status, joinScript: {UdmuxEndpoints, MachineAddress, ...}}`).
pub async fn join_instance(
    c: &RobloxClient,
    place_id: u64,
    job_id: &str,
    cookie: &Secret,
) -> Result<Value> {
    let body = serde_json::json!({
        "placeId": place_id,
        "gameId": job_id,
        "isTeleport": false,
        "gameJoinAttemptId": crate::accounts::new_id(),
    });
    c.post_json("gamejoin", "/v1/join-game-instance", &body, Some(cookie))
        .await
}

/// Server IP from a join answer: first UDMUX endpoint, else the machine address.
pub fn server_ip(v: &Value) -> Option<std::net::IpAddr> {
    let js = v.get("joinScript")?;
    js.pointer("/UdmuxEndpoints/0/Address")
        .or_else(|| js.get("MachineAddress"))
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::post, Json, Router};
    use serde_json::json;

    #[test]
    fn ip_prefers_udmux_then_machine_and_rejects_garbage() {
        let j = |s: Value| json!({"status": 2, "joinScript": s});
        let ip = |v| server_ip(&v).map(|i| i.to_string());
        assert_eq!(
            ip(j(
                json!({"UdmuxEndpoints": [{"Address": "1.2.3.4", "Port": 1}], "MachineAddress": "10.0.0.1"})
            )),
            Some("1.2.3.4".into())
        );
        assert_eq!(
            ip(j(
                json!({"UdmuxEndpoints": [], "MachineAddress": "10.0.0.1"})
            )),
            Some("10.0.0.1".into())
        );
        assert_eq!(ip(j(json!({"MachineAddress": "evil.example/x"}))), None);
        assert_eq!(ip(json!({"status": 12, "joinScript": null})), None);
    }

    #[tokio::test]
    async fn posts_place_and_job() {
        let app = Router::new().route(
            "/v1/join-game-instance",
            post(|Json(b): Json<Value>| async move {
                assert_eq!(
                    (b["placeId"].as_u64(), b["gameId"].as_str()),
                    (Some(5), Some("j"))
                );
                Json(json!({"status": 2, "joinScript": {"MachineAddress": "1.1.1.1"}}))
            }),
        );
        let c = crate::roblox::mock(app).await;
        let v = join_instance(&c, 5, "j", &Secret::new("c")).await.unwrap();
        assert_eq!(server_ip(&v).unwrap().to_string(), "1.1.1.1");
    }
}

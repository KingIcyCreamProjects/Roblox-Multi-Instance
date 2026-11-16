//! Wire types shared by the WS control server and the HTTP API (spec §9).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// One WS frame: `{ "t": "<type>", "id"?: <corr id>, "data": {...} }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Msg {
    pub t: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(default)]
    pub data: Value,
}

impl Msg {
    pub fn new(t: &str, id: Option<Value>, data: Value) -> Self {
        Msg {
            t: t.into(),
            id,
            data,
        }
    }
}

/// `{error:{code,message}}` with an HTTP status.
#[derive(Debug, Clone, PartialEq)]
pub struct ApiError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn new(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        ApiError {
            status,
            code,
            message: message.into(),
        }
    }
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(404, "not_found", message)
    }
    pub fn body(&self) -> Value {
        json!({ "error": { "code": self.code, "message": self.message } })
    }
}

/// Fresh API token: 32 random bytes as 64 hex chars.
pub fn new_token() -> String {
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).expect("OS random source unavailable");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_64_hex_and_random() {
        let (a, b) = (new_token(), new_token());
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn msg_roundtrip_and_defaults() {
        let m: Msg = serde_json::from_str(r#"{"t":"ping"}"#).unwrap();
        assert_eq!(m, Msg::new("ping", None, Value::Null));
        let s = serde_json::to_string(&Msg::new("pong", Some(json!("7")), json!({}))).unwrap();
        assert_eq!(s, r#"{"t":"pong","id":"7","data":{}}"#);
    }
}

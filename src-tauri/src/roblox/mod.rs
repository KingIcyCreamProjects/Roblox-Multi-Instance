//! Roblox HTTP client core. No tauri dependency; commands live elsewhere.
pub mod api;
pub mod client;
pub mod ratelimit;

use std::fmt;
use zeroize::Zeroizing;

pub use client::{BaseUrls, RobloxClient};

/// `.ROBLOSECURITY` value. Debug is masked; no Display on purpose.
pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(v: impl Into<String>) -> Self {
        Self(Zeroizing::new(v.into()))
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RobloxError {
    #[error("Can't reach Roblox: {0}")]
    Network(String),
    #[error("Roblox returned an error (HTTP {code}). Try again later.")]
    Status { code: u16, body_snippet: String },
    #[error("Session expired, sign in again")]
    Unauthorized,
    #[error("Roblox is rate-limiting us, try again later")]
    RateLimited,
    #[error("Unexpected response from Roblox: {0}")]
    Decode(String),
    /// Plain words for the user (a 200 reply that still means "no").
    #[error("{0}")]
    Message(String),
    /// 403 with `rblx-challenge-*` headers (captcha, 2-step...): the challenge type.
    #[error("Roblox asked for an extra check ({0}).")]
    Challenge(String),
}

pub type Result<T> = std::result::Result<T, RobloxError>;

/// Serve `app` on a local port; returns the base URL (every Roblox host points there in tests).
#[cfg(test)]
pub(crate) async fn mock_url(app: axum::Router) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    url
}

#[cfg(test)]
pub(crate) async fn mock(app: axum::Router) -> RobloxClient {
    RobloxClient::new(BaseUrls::all(&mock_url(app).await))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_debug_masked() {
        let s = Secret::new("hunter2");
        assert_eq!(format!("{s:?}"), "Secret(***)");
        assert_eq!(format!("{:?}", Some(&s)), "Some(Secret(***))");
    }
}

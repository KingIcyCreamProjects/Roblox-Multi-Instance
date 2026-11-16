use super::{ratelimit::RateLimiter, Result, RobloxError, Secret};
use reqwest::{header::HeaderValue, Method, StatusCode};
use serde::de::DeserializeOwned;
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    sync::{Arc, Mutex},
    time::Duration,
};

pub const HOSTS: [&str; 13] = [
    "users",
    "presence",
    "thumbnails",
    "auth",
    "games",
    "apis",
    "www",
    "friends",
    "avatar",
    "accountsettings",
    "economy",
    "gamejoin",
    "develop",
];
const MAX_TRIES: u32 = 5;
const HOST_PER_MIN: u32 = 60;
/// Spec 8.1 per-account cap on authenticated calls, per host.
const ACCOUNT_PER_MIN: u32 = 20;
const MAX_RETRY_AFTER_MS: u64 = 60_000;

#[derive(Clone)]
pub struct BaseUrls(HashMap<&'static str, String>);

impl BaseUrls {
    pub fn prod() -> Self {
        Self(
            HOSTS
                .iter()
                .map(|k| (*k, format!("https://{k}.roblox.com")))
                .collect(),
        )
    }
    /// Every host key -> one URL (tests).
    pub fn all(url: &str) -> Self {
        Self(
            HOSTS
                .iter()
                .map(|k| (*k, url.trim_end_matches('/').to_string()))
                .collect(),
        )
    }
}

/// Called with `(host, retry_in_ms)` on every 429 backoff.
pub type RateLimitHook = Arc<dyn Fn(&str, u64) + Send + Sync>;

pub struct RobloxClient {
    http: reqwest::Client,
    /// Images: no redirects (a CDN URL must not bounce elsewhere). Also `post_for_session`.
    cdn: reqwest::Client,
    base: BaseUrls,
    limiter: RateLimiter,
    /// CSRF token per cookie, keyed by a hash of the cookie (key 0 = no cookie).
    csrf: Mutex<HashMap<u64, String>>,
    hook: Mutex<Option<RateLimitHook>>,
    /// Host → no request before this instant (set by a 429). Shared, so N callers wait out
    /// one backoff together instead of each hitting the host again on its own timer.
    cooldown: Mutex<HashMap<String, tokio::time::Instant>>,
}

fn cookie_key(c: Option<&Secret>) -> u64 {
    c.map_or(0, |s| {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        s.expose().hash(&mut h);
        h.finish()
    })
}

impl RobloxClient {
    pub fn new(base: BaseUrls) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("reqwest client builds"),
            cdn: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("reqwest client builds"),
            base,
            limiter: RateLimiter::default(),
            csrf: Mutex::default(),
            hook: Mutex::default(),
            cooldown: Mutex::default(),
        }
    }

    /// Sleeps out the host's shared 429 cooldown, if any.
    async fn wait_cooldown(&self, host: &str) {
        let until = self.cooldown.lock().unwrap().get(host).copied();
        if let Some(t) = until.filter(|t| *t > tokio::time::Instant::now()) {
            tokio::time::sleep_until(t).await;
        }
    }

    /// Pushes the host's cooldown out to `now + ms` (never pulls it in).
    fn back_off(&self, host: &str, ms: u64) {
        let t = tokio::time::Instant::now() + Duration::from_millis(ms);
        let mut g = self.cooldown.lock().unwrap();
        let e = g.entry(host.to_owned()).or_insert(t);
        *e = (*e).max(t);
    }

    pub fn set_ratelimit_hook(&self, h: RateLimitHook) {
        *self.hook.lock().unwrap() = Some(h);
    }

    pub async fn get_json<T: DeserializeOwned>(
        &self,
        host: &str,
        path: &str,
        cookie: Option<&Secret>,
    ) -> Result<T> {
        decode(
            self.send(Method::GET, host, path, None, &[], cookie)
                .await?,
        )
        .await
    }

    pub async fn post_json<T: DeserializeOwned>(
        &self,
        host: &str,
        path: &str,
        body: &serde_json::Value,
        cookie: Option<&Secret>,
    ) -> Result<T> {
        decode(
            self.send(Method::POST, host, path, Some(body), &[], cookie)
                .await?,
        )
        .await
    }

    /// Authenticated POST whose answer is a response header (e.g. the auth ticket).
    pub async fn post_for_header(
        &self,
        host: &str,
        path: &str,
        headers: &[(&str, &str)],
        want: &str,
        cookie: &Secret,
    ) -> Result<String> {
        let r = self
            .send(
                Method::POST,
                host,
                path,
                Some(&serde_json::json!({})),
                headers,
                Some(cookie),
            )
            .await?;
        r.headers()
            .get(want)
            .and_then(|v| v.to_str().ok())
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| RobloxError::Decode(format!("missing {want} header")))
    }

    /// JSON from a non-Roblox URL (geo lookup). Never sends a cookie; http(s) only;
    /// own rate-limit bucket `key`.
    pub async fn get_external_json(
        &self,
        url: &str,
        key: &str,
        per_min: u32,
    ) -> Result<serde_json::Value> {
        let u = reqwest::Url::parse(url).map_err(|e| RobloxError::Decode(e.to_string()))?;
        if !matches!(u.scheme(), "http" | "https") {
            return Err(RobloxError::Decode("lookup URL must be http(s)".into()));
        }
        self.limiter.acquire(key, per_min).await;
        let r = self
            .cdn
            .get(u)
            .send()
            .await
            .map_err(|e| RobloxError::Network(e.without_url().to_string()))?;
        if !r.status().is_success() {
            return Err(RobloxError::Status {
                code: r.status().as_u16(),
                body_snippet: String::new(),
            });
        }
        let b = capped(r, 64 << 10, "lookup answer too large").await?;
        serde_json::from_slice(&b).map_err(|e| RobloxError::Decode(e.to_string()))
    }

    pub async fn patch_json<T: DeserializeOwned>(
        &self,
        host: &str,
        path: &str,
        body: &serde_json::Value,
        cookie: Option<&Secret>,
    ) -> Result<T> {
        decode(
            self.send(Method::PATCH, host, path, Some(body), &[], cookie)
                .await?,
        )
        .await
    }

    /// POST that may rotate the session: also returns a new `.ROBLOSECURITY` from `Set-Cookie`.
    pub async fn post_json_rotating<T: DeserializeOwned>(
        &self,
        host: &str,
        path: &str,
        body: &serde_json::Value,
        cookie: &Secret,
    ) -> Result<(T, Option<Secret>)> {
        let r = self
            .send(Method::POST, host, path, Some(body), &[], Some(cookie))
            .await?;
        let n = new_session_cookie(r.headers());
        Ok((decode(r).await?, n))
    }

    /// Unauthenticated POST that signs in: the new `.ROBLOSECURITY` from `Set-Cookie`, if any.
    /// No redirects (a 307/308 would resend the body, which may hold a secret): 3xx is an error.
    pub async fn post_for_session(
        &self,
        host: &str,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<Option<Secret>> {
        let r = self
            .send_with(&self.cdn, Method::POST, host, path, Some(body), &[], None)
            .await?;
        Ok(new_session_cookie(r.headers()))
    }

    /// Image bytes from the Roblox CDN (https `*.rbxcdn.com` only, no cookie, ≤ 4 MB).
    pub async fn get_cdn(&self, url: &str) -> Result<Vec<u8>> {
        let u = reqwest::Url::parse(url).map_err(|e| RobloxError::Decode(e.to_string()))?;
        if u.scheme() != "https" || !u.host_str().is_some_and(|h| h.ends_with(".rbxcdn.com")) {
            return Err(RobloxError::Decode("unexpected image host".into()));
        }
        self.limiter.acquire("cdn", 120).await;
        let r = self
            .cdn
            .get(u)
            .send()
            .await
            .map_err(|e| RobloxError::Network(e.without_url().to_string()))?;
        if !r.status().is_success() {
            return Err(RobloxError::Status {
                code: r.status().as_u16(),
                body_snippet: String::new(),
            });
        }
        capped(r, 4 << 20, "image too large").await
    }

    async fn send(
        &self,
        method: Method,
        host: &str,
        path: &str,
        body: Option<&serde_json::Value>,
        headers: &[(&str, &str)],
        cookie: Option<&Secret>,
    ) -> Result<reqwest::Response> {
        self.send_with(&self.http, method, host, path, body, headers, cookie)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn send_with(
        &self,
        http: &reqwest::Client,
        method: Method,
        host: &str,
        path: &str,
        body: Option<&serde_json::Value>,
        headers: &[(&str, &str)],
        cookie: Option<&Secret>,
    ) -> Result<reqwest::Response> {
        let base = self
            .base
            .0
            .get(host)
            .unwrap_or_else(|| panic!("unknown roblox host key {host}"));
        let url = format!("{base}{path}");
        let key = cookie_key(cookie);
        let mut csrf_retried = false;
        let mut tries = 0u32;
        loop {
            self.wait_cooldown(host).await;
            // account bucket first: waiting on it must not hold a spent host token
            if cookie.is_some() {
                self.limiter
                    .acquire(&format!("{host}#{key:x}"), ACCOUNT_PER_MIN)
                    .await;
            }
            self.limiter.acquire(host, HOST_PER_MIN).await;
            let mut rb = http.request(method.clone(), &url);
            if let Some(c) = cookie {
                let mut v = HeaderValue::from_str(&format!(".ROBLOSECURITY={}", c.expose()))
                    .map_err(|_| RobloxError::Unauthorized)?;
                v.set_sensitive(true);
                rb = rb.header(reqwest::header::COOKIE, v);
            }
            if method != Method::GET {
                if let Some(t) = self.csrf.lock().unwrap().get(&key) {
                    rb = rb.header("x-csrf-token", t);
                }
            }
            for (k, v) in headers {
                rb = rb.header(*k, *v);
            }
            if let Some(b) = body {
                rb = rb.json(b);
            }
            let resp = rb
                .send()
                .await
                .map_err(|e| RobloxError::Network(e.without_url().to_string()))?;
            let status = resp.status();
            // GET never carries the token, so a retry would only repeat the 403.
            if status == StatusCode::FORBIDDEN && !csrf_retried && method != Method::GET {
                if let Some(t) = resp
                    .headers()
                    .get("x-csrf-token")
                    .and_then(|v| v.to_str().ok())
                {
                    self.csrf.lock().unwrap().insert(key, t.to_string());
                    csrf_retried = true;
                    continue;
                }
            }
            if status == StatusCode::TOO_MANY_REQUESTS {
                tries += 1;
                if tries >= MAX_TRIES {
                    return Err(RobloxError::RateLimited);
                }
                let retry_after = resp
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .map_or(0, |s| (s * 1000).min(MAX_RETRY_AFTER_MS));
                let ms = (500u64 << (tries - 1)).min(30_000).max(retry_after)
                    + rand::random_range(0..250u64);
                let hook = self.hook.lock().unwrap().clone();
                if let Some(h) = hook {
                    h(host, ms);
                }
                self.back_off(host, ms); // the loop top sleeps it out
                csrf_retried = false; // a fresh attempt may need a fresh token
                continue;
            }
            if status == StatusCode::UNAUTHORIZED && cookie.is_some() {
                self.csrf.lock().unwrap().remove(&key); // dead cookie: its token too
                return Err(RobloxError::Unauthorized);
            }
            if status == StatusCode::FORBIDDEN {
                let h = resp.headers();
                if h.contains_key("rblx-challenge-id") || h.contains_key("rblx-challenge-type") {
                    let t = h
                        .get("rblx-challenge-type")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("unknown");
                    return Err(RobloxError::Challenge(t.chars().take(32).collect()));
                }
            }
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                return Err(RobloxError::Status {
                    code: status.as_u16(),
                    body_snippet: text.chars().take(200).collect(),
                });
            }
            return Ok(resp);
        }
    }
}

/// Body bytes, `Decode(too_big)` past `cap`.
async fn capped(mut r: reqwest::Response, cap: usize, too_big: &str) -> Result<Vec<u8>> {
    let net = |e: reqwest::Error| RobloxError::Network(e.without_url().to_string());
    if r.content_length().is_some_and(|n| n > cap as u64) {
        return Err(RobloxError::Decode(too_big.into()));
    }
    let mut out = Vec::new();
    while let Some(c) = r.chunk().await.map_err(net)? {
        if out.len() + c.len() > cap {
            return Err(RobloxError::Decode(too_big.into()));
        }
        out.extend_from_slice(&c);
    }
    Ok(out)
}

/// Empty body (some Roblox writes reply with nothing) decodes as `null`.
async fn decode<T: DeserializeOwned>(r: reqwest::Response) -> Result<T> {
    let b = capped(r, 4 << 20, "Roblox reply too large.").await?;
    serde_json::from_slice(if b.is_empty() { b"null" } else { &b })
        .map_err(|e| RobloxError::Decode(e.to_string()))
}

fn new_session_cookie(h: &reqwest::header::HeaderMap) -> Option<Secret> {
    h.get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|v| v.strip_prefix(".ROBLOSECURITY="))
        .map(|v| v.split(';').next().unwrap_or_default())
        .find(|v| !v.is_empty())
        .map(Secret::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roblox::api::{presence, thumbnails, users};
    use axum::{
        extract::State,
        http::{HeaderMap, StatusCode},
        response::IntoResponse,
        routing::{any, get, post},
        Json, Router,
    };
    use serde_json::{json, Value};
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

    #[derive(Default)]
    struct Mock {
        hits: AtomicUsize,
        sizes: Mutex<Vec<usize>>,
        cookies: Mutex<Vec<String>>,
    }

    async fn serve() -> (RobloxClient, Arc<Mock>) {
        let m = Arc::new(Mock::default());
        let app = Router::new()
            // 403 + token until the client echoes "tok"
            .route(
                "/csrf",
                any(|State(m): State<Arc<Mock>>, h: HeaderMap| async move {
                    m.hits.fetch_add(1, SeqCst);
                    if h.get("x-csrf-token").is_some_and(|v| v == "tok") {
                        (StatusCode::OK, Json(json!({"ok": true}))).into_response()
                    } else {
                        (StatusCode::FORBIDDEN, [("x-csrf-token", "tok")]).into_response()
                    }
                }),
            )
            // 429 on first hit only
            .route(
                "/flaky",
                get(|State(m): State<Arc<Mock>>| async move {
                    if m.hits.fetch_add(1, SeqCst) == 0 {
                        StatusCode::TOO_MANY_REQUESTS.into_response()
                    } else {
                        Json(json!({"ok": true})).into_response()
                    }
                }),
            )
            .route(
                "/always429",
                get(|State(m): State<Arc<Mock>>| async move {
                    m.hits.fetch_add(1, SeqCst);
                    StatusCode::TOO_MANY_REQUESTS
                }),
            )
            .route(
                "/ra",
                get(|State(m): State<Arc<Mock>>| async move {
                    if m.hits.fetch_add(1, SeqCst) == 0 {
                        (StatusCode::TOO_MANY_REQUESTS, [("retry-after", "3")]).into_response()
                    } else {
                        Json(json!({"ok": true})).into_response()
                    }
                }),
            )
            .route("/always401", get(|| async { StatusCode::UNAUTHORIZED }))
            .route(
                "/v1/usernames/users",
                post(|State(m): State<Arc<Mock>>, Json(b): Json<Value>| async move {
                    m.sizes
                        .lock()
                        .unwrap()
                        .push(b["usernames"].as_array().unwrap().len());
                    Json(json!({"data": [{"requestedUsername": "Bob", "hasVerifiedBadge": false, "id": 7, "name": "bob", "displayName": "Bob"}]}))
                }),
            )
            .route(
                "/v1/users/avatar-headshot",
                get(|| async {
                    Json(json!({"data": [{"targetId": 7, "state": "Completed", "imageUrl": "https://tr.rbxcdn.com/x", "version": "TN3"}, {"targetId": 8, "state": "Pending", "imageUrl": null, "version": ""}]}))
                }),
            )
            .route(
                "/v1/users/avatar",
                get(|| async {
                    Json(json!({"data": [{"targetId": 7, "state": "Completed", "imageUrl": "https://tr.rbxcdn.com/y"}]}))
                }),
            )
            .route(
                "/v1/users/authenticated",
                get(|State(m): State<Arc<Mock>>, h: HeaderMap| async move {
                    match h.get("cookie").and_then(|v| v.to_str().ok()) {
                        Some(c) if c.contains("good") => {
                            m.cookies.lock().unwrap().push(c.to_string());
                            Json(json!({"id": 7, "name": "bob", "displayName": "Bob"}))
                                .into_response()
                        }
                        _ => StatusCode::UNAUTHORIZED.into_response(),
                    }
                }),
            )
            .route(
                "/v1/presence/users",
                post(
                    |State(m): State<Arc<Mock>>, Json(b): Json<Value>| async move {
                        let ids = b["userIds"].as_array().unwrap();
                        m.sizes.lock().unwrap().push(ids.len());
                        let ps: Vec<_> = ids
                            .iter()
                            .map(|i| json!({"userId": i, "userPresenceType": 1}))
                            .collect();
                        Json(json!({"userPresences": ps}))
                    },
                ),
            )
            .with_state(m.clone());
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        (RobloxClient::new(BaseUrls::all(&url)), m)
    }

    #[tokio::test]
    async fn csrf_retry_sends_token() {
        let (c, m) = serve().await;
        let ck = Secret::new("good");
        let v: Value = c
            .post_json("www", "/csrf", &json!({}), Some(&ck))
            .await
            .unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(m.hits.load(SeqCst), 2);
        // token cached: next call succeeds first try
        c.post_json::<Value>("www", "/csrf", &json!({}), Some(&ck))
            .await
            .unwrap();
        assert_eq!(m.hits.load(SeqCst), 3);
        // GET never sends the token: no pointless retry of the 403
        assert!(c
            .get_json::<Value>("www", "/csrf", Some(&ck))
            .await
            .is_err());
        assert_eq!(m.hits.load(SeqCst), 4);
    }

    #[tokio::test(start_paused = true)]
    #[cfg_attr(
        target_os = "macos",
        ignore = "the paused clock outruns loopback sockets on macOS and the request times out"
    )]
    async fn retries_429_then_ok_and_fires_hook() {
        let (c, m) = serve().await;
        let seen = Arc::new(Mutex::new(vec![]));
        let s2 = seen.clone();
        c.set_ratelimit_hook(Arc::new(move |h, ms| {
            s2.lock().unwrap().push((h.to_string(), ms))
        }));
        let v: Value = c.get_json("www", "/flaky", None).await.unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(m.hits.load(SeqCst), 2);
        let s = seen.lock().unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].0, "www");
        assert!((500..750).contains(&s[0].1));
    }

    #[tokio::test(start_paused = true)]
    #[cfg_attr(
        target_os = "macos",
        ignore = "the paused clock outruns loopback sockets on macOS and the request times out"
    )]
    async fn a_429_cools_the_host_down_for_every_caller() {
        let (c, m) = serve().await;
        c.back_off("www", 2_000); // someone else's 429
        c.back_off("www", 500); // never pulls it in
        m.hits.store(1, SeqCst); // past the one 429
        let t = tokio::time::Instant::now();
        c.get_json::<Value>("users", "/flaky", None).await.unwrap(); // other host: no wait
        assert!(t.elapsed() < Duration::from_millis(100));
        c.get_json::<Value>("www", "/flaky", None).await.unwrap();
        assert!(t.elapsed() >= Duration::from_secs(2));
    }

    #[tokio::test(start_paused = true)]
    #[cfg_attr(
        target_os = "macos",
        ignore = "the paused clock outruns loopback sockets on macOS and the request times out"
    )]
    async fn honours_retry_after() {
        let (c, _m) = serve().await;
        let seen = Arc::new(Mutex::new(vec![]));
        let s2 = seen.clone();
        c.set_ratelimit_hook(Arc::new(move |_, ms| s2.lock().unwrap().push(ms)));
        let t = tokio::time::Instant::now();
        c.get_json::<Value>("www", "/ra", None).await.unwrap();
        assert!((3000..3250).contains(&seen.lock().unwrap()[0]));
        assert!(t.elapsed() >= Duration::from_secs(3));
    }

    #[tokio::test(start_paused = true)]
    #[cfg_attr(
        target_os = "macos",
        ignore = "the paused clock outruns loopback sockets on macOS and the request times out"
    )]
    async fn gives_up_after_five_429s() {
        let (c, m) = serve().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let c2 = calls.clone();
        c.set_ratelimit_hook(Arc::new(move |_, _| {
            c2.fetch_add(1, SeqCst);
        }));
        let e = c
            .get_json::<Value>("www", "/always429", None)
            .await
            .unwrap_err();
        assert!(matches!(e, RobloxError::RateLimited));
        assert_eq!(m.hits.load(SeqCst), 5);
        assert_eq!(calls.load(SeqCst), 4);
    }

    #[tokio::test(start_paused = true)]
    #[cfg_attr(
        target_os = "macos",
        ignore = "the paused clock outruns loopback sockets on macOS and the request times out"
    )]
    async fn per_account_cap_20_per_min() {
        let (c, _m) = serve().await;
        let t = tokio::time::Instant::now();
        for _ in 0..20 {
            users::authenticated(&c, &Secret::new("good"))
                .await
                .unwrap();
        }
        users::authenticated(&c, &Secret::new("good2"))
            .await
            .unwrap(); // other account free
                       // Paused time auto-advances while real sockets are idle (slow CI runners), so only
                       // check that no 3 s cap wait happened yet.
        assert!(t.elapsed() < Duration::from_millis(2900));
        users::authenticated(&c, &Secret::new("good"))
            .await
            .unwrap();
        assert!(t.elapsed() >= Duration::from_millis(2900));
    }

    #[tokio::test]
    async fn status_401_without_cookie_is_status() {
        let (c, _m) = serve().await;
        let e = c
            .get_json::<Value>("www", "/always401", None)
            .await
            .unwrap_err();
        assert!(matches!(e, RobloxError::Status { code: 401, .. }));
        let e = c
            .get_json::<Value>("www", "/always401", Some(&Secret::new("x")))
            .await
            .unwrap_err();
        assert!(matches!(e, RobloxError::Unauthorized));
    }

    #[tokio::test]
    async fn invalid_cookie_is_unauthorized() {
        let (c, _m) = serve().await;
        let e = c
            .get_json::<Value>(
                "www",
                "/flaky",
                Some(&Secret::new(
                    "bad
value",
                )),
            )
            .await
            .unwrap_err();
        assert!(matches!(e, RobloxError::Unauthorized));
    }

    #[tokio::test]
    async fn decodes_usernames_and_chunks() {
        let (c, m) = serve().await;
        let names: Vec<String> = (0..150).map(|i| format!("u{i}")).collect();
        let r = users::by_usernames(&c, &names).await.unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!((r[0].id, r[0].requested_username.as_str()), (7, "Bob"));
        assert_eq!(*m.sizes.lock().unwrap(), vec![100, 50]);
    }

    #[tokio::test]
    async fn decodes_thumbnails() {
        let (c, _m) = serve().await;
        let h = thumbnails::headshots(&c, &[7, 8], "48x48").await.unwrap();
        assert_eq!(h[0].image_url.as_deref(), Some("https://tr.rbxcdn.com/x"));
        assert_eq!(
            (h[1].state.as_str(), h[1].image_url.clone()),
            ("Pending", None)
        );
        let a = thumbnails::avatar(&c, 7, "420x420").await.unwrap().unwrap();
        assert_eq!(a.target_id, 7);
    }

    #[tokio::test]
    async fn unauthorized_on_401_and_cookie_sent() {
        let (c, m) = serve().await;
        let e = users::authenticated(&c, &Secret::new("bad"))
            .await
            .unwrap_err();
        assert!(matches!(e, RobloxError::Unauthorized));
        let u = users::authenticated(&c, &Secret::new("good"))
            .await
            .unwrap();
        assert_eq!(
            (u.id, u.name.as_str(), u.display_name.as_str()),
            (7, "bob", "Bob")
        );
        assert_eq!(m.cookies.lock().unwrap()[0], ".ROBLOSECURITY=good");
    }

    #[tokio::test]
    async fn presence_chunks_of_100() {
        let (c, m) = serve().await;
        let ids: Vec<u64> = (1..=250).collect();
        let out = presence::get(&c, &ids, None).await.unwrap();
        assert_eq!(out.len(), 250);
        assert_eq!(*m.sizes.lock().unwrap(), vec![100, 100, 50]);
    }

    #[tokio::test]
    async fn external_json_capped_at_64k() {
        let app = Router::new()
            .route("/small", get(|| async { Json(json!({"city": "X"})) }))
            .route("/big", get(|| async { "x".repeat(70_000) }));
        let u = crate::roblox::mock_url(app).await;
        let c = RobloxClient::new(BaseUrls::all(&u));
        let ok = c.get_external_json(&format!("{u}/small"), "geo", 40).await;
        assert_eq!(ok.unwrap()["city"], "X");
        let big = c.get_external_json(&format!("{u}/big"), "geo", 40).await;
        assert!(matches!(big, Err(RobloxError::Decode(m)) if m.contains("too large")));
    }

    #[test]
    fn session_cookie_taken_from_set_cookie() {
        use reqwest::header::{HeaderMap, HeaderValue, SET_COOKIE};
        let mut h = HeaderMap::new();
        h.append(
            SET_COOKIE,
            HeaderValue::from_static(".ROBLOSECURITY=; expires=Thu, 01 Jan 1970 00:00:00 GMT"),
        );
        h.append(SET_COOKIE, HeaderValue::from_static("other=1; path=/"));
        h.append(
            SET_COOKIE,
            HeaderValue::from_static(".ROBLOSECURITY=NEW; HttpOnly"),
        );
        assert_eq!(new_session_cookie(&h).unwrap().expose(), "NEW");
        let mut cleared = HeaderMap::new();
        cleared.append(
            SET_COOKIE,
            HeaderValue::from_static(".ROBLOSECURITY=; expires=x"),
        );
        assert!(new_session_cookie(&cleared).is_none());
    }

    #[test]
    fn base_urls_trim_and_prod_hosts() {
        assert_eq!(BaseUrls::all("http://x:1/").0["gamejoin"], "http://x:1");
        assert_eq!(
            BaseUrls::prod().0["gamejoin"],
            "https://gamejoin.roblox.com"
        );
    }

    #[tokio::test]
    async fn empty_body_decodes_as_null() {
        let app = Router::new().route("/empty", post(|| async { StatusCode::NO_CONTENT }));
        let c = RobloxClient::new(BaseUrls::all(&crate::roblox::mock_url(app).await));
        let v: Option<Value> = c
            .post_json("www", "/empty", &json!({}), None)
            .await
            .unwrap();
        assert_eq!(v, None);
    }

    #[tokio::test]
    async fn header_answer_or_missing_header_error() {
        let app = Router::new()
            .route(
                "/tick",
                post(|| async { (StatusCode::OK, [("rbx-authentication-ticket", "T1")]) }),
            )
            .route("/none", post(|| async { StatusCode::OK }));
        let c = RobloxClient::new(BaseUrls::all(&crate::roblox::mock_url(app).await));
        let ck = Secret::new("good");
        let t = c
            .post_for_header("www", "/tick", &[], "rbx-authentication-ticket", &ck)
            .await
            .unwrap();
        assert_eq!(t, "T1");
        let e = c
            .post_for_header("www", "/none", &[], "rbx-authentication-ticket", &ck)
            .await
            .unwrap_err();
        assert!(matches!(e, RobloxError::Decode(m) if m.contains("missing")));
    }

    #[tokio::test]
    async fn forbidden_without_token_is_status() {
        let app = Router::new().route("/nope", post(|| async { StatusCode::FORBIDDEN }));
        let c = RobloxClient::new(BaseUrls::all(&crate::roblox::mock_url(app).await));
        let e = c
            .post_json::<Value>("www", "/nope", &json!({}), Some(&Secret::new("good")))
            .await
            .unwrap_err();
        assert!(matches!(e, RobloxError::Status { code: 403, .. }));
    }

    #[tokio::test(start_paused = true)]
    #[cfg_attr(
        target_os = "macos",
        ignore = "the paused clock outruns loopback sockets on macOS and the request times out"
    )]
    async fn retry_after_is_capped_at_a_minute() {
        let hits = Arc::new(AtomicUsize::new(0));
        let h2 = hits.clone();
        let app = Router::new().route(
            "/slow",
            get(move || {
                let h2 = h2.clone();
                async move {
                    if h2.fetch_add(1, SeqCst) == 0 {
                        (StatusCode::TOO_MANY_REQUESTS, [("retry-after", "900")]).into_response()
                    } else {
                        Json(json!({"ok": true})).into_response()
                    }
                }
            }),
        );
        let c = RobloxClient::new(BaseUrls::all(&crate::roblox::mock_url(app).await));
        let seen = Arc::new(Mutex::new(vec![]));
        let s2 = seen.clone();
        c.set_ratelimit_hook(Arc::new(move |_, ms| s2.lock().unwrap().push(ms)));
        c.get_json::<Value>("www", "/slow", None).await.unwrap();
        assert!((60_000..60_250).contains(&seen.lock().unwrap()[0]));
    }

    #[tokio::test]
    async fn refused_connection_is_network_error() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        drop(l);
        let c = RobloxClient::new(BaseUrls::all(&url));
        let e = c.get_json::<Value>("www", "/x", None).await.unwrap_err();
        assert!(matches!(e, RobloxError::Network(_)));
    }

    #[tokio::test]
    async fn external_lookup_rules() {
        let app = Router::new().route("/nf", get(|| async { StatusCode::NOT_FOUND }));
        let u = crate::roblox::mock_url(app).await;
        let c = RobloxClient::new(BaseUrls::all(&u));
        let e = c
            .get_external_json("ftp://x.invalid/a", "geo", 40)
            .await
            .unwrap_err();
        assert!(matches!(e, RobloxError::Decode(m) if m.contains("http(s)")));
        let e = c
            .get_external_json("not a url", "geo", 40)
            .await
            .unwrap_err();
        assert!(matches!(e, RobloxError::Decode(_)));
        let e = c
            .get_external_json(&format!("{u}/nf"), "geo", 40)
            .await
            .unwrap_err();
        assert!(matches!(e, RobloxError::Status { code: 404, .. }));
    }

    #[tokio::test]
    async fn cdn_only_https_rbxcdn() {
        // all rejected before any request is sent
        let c = RobloxClient::new(BaseUrls::prod());
        for u in [
            "http://tr.rbxcdn.com/a",
            "https://rbxcdn.com/a",
            "https://evil.com/x.rbxcdn.com",
            "https://rbxcdn.com.evil.io/a",
            "not a url",
        ] {
            let e = c.get_cdn(u).await.unwrap_err();
            assert!(matches!(e, RobloxError::Decode(_)), "{u}");
        }
    }

    #[tokio::test]
    async fn csrf_token_is_per_cookie() {
        let (c, m) = serve().await;
        c.post_json::<Value>("www", "/csrf", &json!({}), Some(&Secret::new("a")))
            .await
            .unwrap();
        assert_eq!(m.hits.load(SeqCst), 2);
        // another account's token is not sent: it gets its own 403 round trip
        c.post_json::<Value>("www", "/csrf", &json!({}), Some(&Secret::new("b")))
            .await
            .unwrap();
        assert_eq!(m.hits.load(SeqCst), 4);
    }

    #[tokio::test]
    async fn csrf_second_403_is_status_not_a_loop() {
        let app = Router::new().route(
            "/csrf",
            post(|| async { (StatusCode::FORBIDDEN, [("x-csrf-token", "tok")]) }),
        );
        let c = RobloxClient::new(BaseUrls::all(&crate::roblox::mock_url(app).await));
        let e = c
            .post_json::<Value>("www", "/csrf", &json!({}), Some(&Secret::new("good")))
            .await
            .unwrap_err();
        assert!(matches!(e, RobloxError::Status { code: 403, .. }));
    }

    #[tokio::test]
    async fn external_lookup_does_not_follow_redirects() {
        let app = Router::new()
            .route(
                "/bounce",
                get(|| async { (StatusCode::FOUND, [("location", "/small")]) }),
            )
            .route("/small", get(|| async { Json(json!({"city": "X"})) }));
        let u = crate::roblox::mock_url(app).await;
        let c = RobloxClient::new(BaseUrls::all(&u));
        let e = c
            .get_external_json(&format!("{u}/bounce"), "geo", 40)
            .await
            .unwrap_err();
        assert!(matches!(e, RobloxError::Status { code: 302, .. }));
    }
}

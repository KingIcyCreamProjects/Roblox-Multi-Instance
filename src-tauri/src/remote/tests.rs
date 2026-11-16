//! Host tests: fake desk + recording input over a loopback socket. Nothing here captures the
//! real screen, moves the real mouse or binds a Tailscale address.

use super::*;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message as WsMsg;

struct Rec(Arc<Mutex<Vec<String>>>);

impl Input for Rec {
    fn move_to(&mut self, x: i32, y: i32) {
        lock(&self.0).push(format!("move {x},{y}"));
    }
    fn button(&mut self, b: u8, down: bool) {
        lock(&self.0).push(format!("btn {b} {down}"));
    }
    fn wheel(&mut self, dx: i32, dy: i32) {
        lock(&self.0).push(format!("wheel {dx},{dy}"));
    }
    fn key(&mut self, code: &str, down: bool) {
        lock(&self.0).push(format!("key {code} {down}"));
    }
    fn text(&mut self, s: &str) {
        lock(&self.0).push(format!("text {s}"));
    }
    fn lock(&mut self) {
        lock(&self.0).push("lock".into());
    }
}

/// One 100x50 screen whose every grab differs (so no frame is skipped as identical).
#[derive(Default)]
struct Fake {
    n: AtomicUsize,
    rec: Arc<Mutex<Vec<String>>>,
    same: AtomicBool,
    /// The desk reports no change (DXGI: no new frame).
    still: AtomicBool,
    /// A second monitor plugged in.
    plugged: AtomicBool,
    /// Pointer at (10, 20), shown as text.
    pointer: AtomicBool,
    /// Sound: 6 channels (5.1) instead of stereo.
    surround: AtomicBool,
    /// Sound: capture fails.
    no_sound: AtomicBool,
}

impl Desk for Fake {
    fn os(&self) -> &'static str {
        "windows"
    }
    fn screens(&self) -> Vec<Screen> {
        let s = Screen {
            id: 0,
            x: 0,
            y: 0,
            w: 100,
            h: 50,
            primary: true,
        };
        let n = if self.plugged.load(SeqCst) { 2 } else { 1 };
        (0..n)
            .map(|i| Screen {
                id: i,
                x: i32::from(i) * 100,
                primary: i == 0,
                ..s
            })
            .collect()
    }
    fn grab(&self, _: u8) -> Option<image::RgbaImage> {
        let n = if self.same.load(SeqCst) {
            0
        } else {
            self.n.fetch_add(1, SeqCst)
        };
        Some(image::RgbaImage::from_pixel(
            100,
            50,
            image::Rgba([n as u8, 0, 0, 255]),
        ))
    }
    fn unchanged(&self, _: u8) -> bool {
        self.still.load(SeqCst)
    }
    fn cursor(&self) -> Option<(i32, i32)> {
        self.pointer.load(SeqCst).then_some((10, 20))
    }
    fn cursor_shape(&self) -> Option<&'static str> {
        Some("text")
    }
    fn state(&self) -> (&'static str, bool) {
        ("ready", true)
    }
    fn input(&self) -> Box<dyn Input> {
        Box::new(Rec(self.rec.clone()))
    }
    fn has_audio(&self) -> bool {
        true
    }
    /// 48 kHz stereo, 10 ms of samples (value 1) every 10 ms. Surround: frames 1..=6.
    fn audio(&self, run: &dyn Fn() -> bool, sink: &mut dyn FnMut(audio::Sound)) {
        if self.no_sound.load(SeqCst) {
            return sink(audio::Sound::Fail("No sound here."));
        }
        let six = self.surround.load(SeqCst);
        sink(audio::Sound::Format(48000, if six { 6 } else { 2 }));
        while run() {
            let p = if six {
                [1, 2, 3, 4, 5, 6].repeat(480)
            } else {
                vec![1; 960]
            };
            sink(audio::Sound::Pcm(p));
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn setup(on: bool) -> (tempfile::TempDir, Remote, Arc<Fake>) {
    let dir = tempfile::tempdir().unwrap();
    let s = json!({"remote": {"enabled": on, "files": true, "port": DEFAULT_PORT, "deviceName": " Test PC "}});
    let fake = Arc::new(Fake::default());
    let r = Remote::new(Arc::new(Mutex::new(s)), dir.path(), fake.clone());
    (dir, r, fake)
}

/// Serves `r` on 127.0.0.1 (tests only; the app binds the Tailscale address).
async fn serve_local(r: &Remote) -> (String, oneshot::Sender<()>) {
    let l = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let (tx, mut rx) = oneshot::channel();
    let c = r.c.clone();
    tokio::spawn(async move { serve(c, l, &mut rx).await });
    for _ in 0..50 {
        if lock(&r.c.bound).is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    (addr, tx)
}

async fn pair_with(addr: &str, code: &str) -> (u16, Value) {
    let r = reqwest::Client::new()
        .post(format!("http://{addr}/v1/remote/pair"))
        .json(&json!({"code": code, "name": "Laptop"}))
        .send()
        .await
        .unwrap();
    (r.status().as_u16(), r.json().await.unwrap_or_default())
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn open(addr: &str, first: &str) -> Ws {
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/v1/remote/ws"))
        .await
        .unwrap();
    ws.send(WsMsg::text(first)).await.unwrap();
    ws
}

/// Next binary message within `ms`, `None` on timeout or close.
async fn next(ws: &mut Ws, ms: u64) -> Option<Vec<u8>> {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
    loop {
        match tokio::time::timeout_at(deadline, ws.next()).await {
            Ok(Some(Ok(WsMsg::Binary(b)))) => return Some(b.to_vec()),
            Ok(Some(Ok(WsMsg::Close(_)))) | Ok(None) | Ok(Some(Err(_))) | Err(_) => return None,
            Ok(Some(Ok(_))) => continue,
        }
    }
}

/// Next frame (`0x01`) within `ms`, skipping control messages.
async fn next_frame(ws: &mut Ws, ms: u64) -> Option<Vec<u8>> {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let b = next(ws, left.as_millis() as u64).await?;
        if b[0] == 0x01 {
            return Some(b);
        }
    }
}

async fn next_ctl(ws: &mut Ws) -> Value {
    loop {
        let b = next(ws, 3000).await.expect("control message");
        if b[0] == 0x02 {
            return serde_json::from_slice(&b[1..]).unwrap();
        }
    }
}

async fn send(ws: &mut Ws, v: Value) {
    ws.send(WsMsg::text(v.to_string())).await.unwrap();
}

async fn paired_token(r: &Remote, addr: &str) -> String {
    let code = r.new_code().unwrap().code;
    let (st, v) = pair_with(addr, &code).await;
    assert_eq!(st, 200, "{v}");
    v["token"].as_str().unwrap().to_owned()
}

fn auth(token: &str) -> String {
    json!({"t": "auth", "token": token}).to_string()
}

#[tokio::test]
async fn running_report_only_from_a_paired_computer() {
    let (_d, r, _f) = setup(true);
    let (addr, _stop) = serve_local(&r).await;
    let token = paired_token(&r, &addr).await;
    let post = |token: Option<String>, body: Value| {
        let mut q = reqwest::Client::new().post(format!("http://{addr}/v1/remote/running"));
        if let Some(t) = token {
            q = q.bearer_auth(t);
        }
        async move { q.json(&body).send().await.unwrap().status().as_u16() }
    };
    let body = json!({"name": " Mac ", "userIds": [7, 9]});
    // No token, or one nobody paired with: refused, nothing stored.
    assert_eq!(post(None, body.clone()).await, 403);
    assert_eq!(post(Some(new_token()), body.clone()).await, 403);
    assert!(r.running_elsewhere().is_empty());
    assert_eq!(post(Some(token.clone()), body).await, 200);
    let mut got = r.running_elsewhere();
    got.sort();
    assert_eq!(got, [(7, "Mac".to_owned()), (9, "Mac".to_owned())]);
    // The next report replaces the last; without a name, the one it paired under.
    let body = json!({"name": "", "userIds": [9]});
    assert_eq!(post(Some(token.clone()), body).await, 200);
    assert_eq!(r.running_elsewhere(), [(9, "Laptop".to_owned())]);
    let many: Vec<u64> = (0..=MAX_RUNNING as u64).collect();
    let body = json!({"name": "Mac", "userIds": many});
    assert_eq!(post(Some(token), body).await, 400);
    // Forgotten once no new report came for `PEER_TTL`.
    for v in lock(&r.c.peers).values_mut() {
        v.0 -= PEER_TTL;
    }
    assert!(r.running_elsewhere().is_empty());
}

/// The next `bye`, skipping other control messages.
async fn bye_of(ws: &mut Ws) -> Value {
    loop {
        let v = next_ctl(ws).await;
        if v["t"] == "bye" {
            return v;
        }
    }
}

/// A second paired device (same test address, so not through `pair`, which would replace
/// the first): its token.
fn other_device(r: &Remote, name: &str) -> String {
    let token = new_token();
    lock(&r.c.store).paired.push(Paired {
        hash: hash(&token),
        name: name.into(),
        paired_at: now_iso(),
        ip: "127.0.0.1".into(),
    });
    token
}

async fn wait_free(r: &Remote) {
    for _ in 0..100 {
        if r.live().is_none() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("session still live");
}

#[test]
fn bind_guard_only_tailscale_range() {
    for ok in ["100.64.0.0", "100.100.7.9", "100.127.255.255"] {
        assert!(bind_ok(ok.parse().unwrap()), "{ok}");
    }
    for bad in [
        "0.0.0.0",
        "127.0.0.1",
        "192.168.1.10",
        "100.63.255.255",
        "100.128.0.0",
        "10.0.0.1",
    ] {
        assert!(!bind_ok(bad.parse().unwrap()), "{bad}");
    }
}

#[test]
fn token_hash_is_sha256_hex() {
    assert_eq!(
        hash("abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert!(valid_token(&new_token()));
    assert!(!valid_token("short"));
}

#[test]
fn messages_decode() {
    let p = |s: &str| serde_json::from_str::<In>(s).unwrap();
    assert_eq!(
        p(r#"{"t":"start"}"#),
        In::Start {
            screen: 0,
            quality: Quality::Auto,
            max_fps: 15,
            cursor: false,
            codecs: vec![]
        }
    );
    assert_eq!(
        p(r#"{"t":"start","screen":1,"quality":"speed","maxFps":5,"cursor":true}"#),
        In::Start {
            screen: 1,
            quality: Quality::Speed,
            max_fps: 5,
            cursor: true,
            codecs: vec![]
        }
    );
    assert_eq!(
        p(r#"{"t":"start","codecs":["h264","av1"]}"#),
        In::Start {
            screen: 0,
            quality: Quality::Auto,
            max_fps: 15,
            cursor: false,
            codecs: vec!["h264".into(), "av1".into()]
        }
    );
    assert_eq!(p(r#"{"t":"idr"}"#), In::Idr);
    assert_eq!(p(r#"{"t":"mm","x":0.5,"y":1}"#), In::Mm { x: 0.5, y: 1.0 });
    assert_eq!(p(r#"{"t":"wh","dy":-120}"#), In::Wh { dx: 0, dy: -120 });
    assert_eq!(
        p(r#"{"t":"kd","code":"KeyA"}"#),
        In::Kd {
            code: "KeyA".into()
        }
    );
    assert_eq!(
        p(r#"{"t":"term","op":"open"}"#),
        In::Other,
        "unknown t ignored"
    );
    assert_eq!(p(r#"{"t":"ping"}"#), In::Ping { n: 0 });
    assert!(serde_json::from_str::<In>(r#"{"x":1}"#).is_err());
    let s = Screen {
        id: 0,
        x: -1920,
        y: 0,
        w: 1920,
        h: 1080,
        primary: false,
    };
    assert_eq!(to_abs(&s, 0.0, 1.0), Some((-1920, 1079)));
    assert_eq!(to_abs(&s, 2.0, -1.0), Some((-1, 0)), "clamped");
    assert_eq!(to_abs(&s, f64::NAN, 0.0), None);
}

#[test]
fn frames_encode_jpeg_and_scale() {
    let enc = |w: u32, q: Quality, level: usize| {
        let (jq, max_w) = q.enc(level, w);
        let f = encode(&image::RgbaImage::new(w, 100), 0, jq, max_w);
        image::load_from_memory(&f[2..]).unwrap().width()
    };
    let f = encode(&image::RgbaImage::new(64, 32), 3, 70, u32::MAX);
    assert_eq!(&f[..4], &[0x01, 3, 0xFF, 0xD8]);
    let big = encode(&image::RgbaImage::new(3000, 100), 0, 50, 1920);
    let img = image::load_from_memory(&big[2..]).unwrap();
    assert_eq!((img.width(), img.height()), (1920, 64));
    assert_eq!(enc(3000, Quality::Quality, 1), 3000);
    assert_eq!(enc(3000, Quality::Speed, 1), 1920);
    // auto: never wider than 1920 (4K, Retina), then the adaptive scale
    assert_eq!(enc(3840, Quality::Auto, 1), 1920);
    assert_eq!(enc(1600, Quality::Auto, 1), 1600);
    assert_eq!(enc(1600, Quality::Auto, 4), 800);
    assert_eq!(Quality::Auto.enc(0, 100).0, 80);
    assert_eq!(
        Quality::Auto.enc(99, 100).0,
        40,
        "clamped to the last level"
    );
}

#[test]
fn auto_quality_follows_acks() {
    let mut t = Tune::default();
    assert_eq!(t.tick(Quality::Auto, 1000)["q"], 70, "starts at 70");
    // a frame nobody acknowledges for over a second: one step down per tick
    t.on_sent(1000);
    t.sent[0] -= Duration::from_secs(2);
    let s = t.tick(Quality::Auto, 1000);
    assert_eq!(
        (s["q"].as_u64(), s["scale"].as_f64()),
        (Some(60), Some(0.75))
    );
    assert_eq!(s["fps"], 1.0, "1 frame in 2 s, rounded");
    assert_eq!(s["kbps"], 4.0);
    assert_eq!((&s["kb"], &s["inflight"]), (&json!(1.0), &json!(1)));
    // calm periods: back up only after CALM_TICKS of them (no flapping)
    t.on_ack();
    for _ in 1..CALM_TICKS {
        t.on_sent(10);
        t.on_ack();
        assert_eq!(t.tick(Quality::Auto, 1000)["q"], 60, "steps up slowly");
    }
    t.on_sent(10);
    t.on_ack();
    assert_eq!(t.tick(Quality::Auto, 1000)["q"], 70);
    // a far but fast link: the window full most of the period at its base round trip
    // (80 ms) is no congestion, and counts as calm
    for _ in 0..MAX_UNACKED {
        t.on_sent(10);
    }
    (t.min_rtt, t.rtt) = (Some(Duration::from_millis(80)), Duration::from_millis(90));
    t.full_since = Some(Instant::now() - Duration::from_millis(1500));
    assert_eq!(t.tick(Quality::Auto, 1000)["q"], 70);
    assert_eq!(t.calm, 1);
    // full for over half the period while acks take over twice the best round trip: down
    t.rtt = Duration::from_millis(200);
    t.full_since = Some(Instant::now() - Duration::from_millis(1500));
    let s = t.tick(Quality::Auto, 1000);
    assert_eq!(
        (s["q"].as_u64(), s["inflight"].as_u64()),
        (Some(60), Some(4))
    );
    // a short full spell is no congestion (and no calm either)
    t.full_since = Some(Instant::now() - Duration::from_millis(500));
    assert_eq!(t.tick(Quality::Auto, 1000)["q"], 60);
    assert_eq!(t.calm, 0);
    // an ack records its round trip; the session minimum only falls
    t.sent[0] -= Duration::from_millis(300);
    t.on_ack();
    assert!(t.rtt >= Duration::from_millis(300));
    assert_eq!(t.min_rtt, Some(Duration::from_millis(80)));
    // fixed qualities never adapt
    t.sent[0] -= Duration::from_secs(5);
    let level = t.level;
    assert_eq!(t.tick(Quality::Quality, 1000)["q"], 85);
    assert_eq!(t.level, level);
    // scale is the real one: the 1920 px cap on a 4K screen comes on top
    t.sent.clear();
    t.full_since = None;
    t.level = 1;
    assert_eq!(t.tick(Quality::Auto, 3840)["scale"], 0.5);
    assert_eq!(t.tick(Quality::Speed, 3840)["scale"], 0.5);
    // capture and encode times, sent width; left out when nothing was captured
    t.on_captured(Duration::from_millis(4));
    t.on_encoded(Duration::from_micros(6250), 1280, None);
    let s = t.tick(Quality::Speed, 3840);
    assert_eq!(
        (&s["cap"], &s["enc"], &s["w"]),
        (&json!(4.0), &json!(6.3), &json!(1280))
    );
    let s = t.tick(Quality::Speed, 3840);
    assert!(s.get("cap").is_none() && s.get("enc").is_none() && s.get("kb").is_none());
}

#[test]
fn identical_frames_are_skipped() {
    let f = Fake::default();
    f.same.store(true, SeqCst);
    let want = Stream {
        screen: 0,
        quality: Quality::Auto,
        fps: 15,
        cursor: false,
        h264: false,
    };
    let mut last = None;
    assert!(snap(&f, &f.screens(), want, &mut last).is_some());
    assert!(snap(&f, &f.screens(), want, &mut last).is_none());
    let other = Stream { fps: 5, ..want };
    assert!(
        snap(&f, &f.screens(), other, &mut last).is_some(),
        "retarget resends"
    );
    let gone = Stream { screen: 9, ..want };
    assert!(snap(&f, &f.screens(), gone, &mut last).is_none());
    // pointer drawn in, unless the viewer draws it (`cursor`)
    f.pointer.store(true, SeqCst);
    let mut last = None;
    let drawn = snap(&f, &f.screens(), want, &mut last).unwrap();
    let mut last = None;
    let bare = Stream {
        cursor: true,
        ..want
    };
    assert_ne!(drawn.1, snap(&f, &f.screens(), bare, &mut last).unwrap().1);
}

#[test]
fn unchanged_desk_skips_the_grab_but_not_a_moved_pointer() {
    let f = Fake::default(); // every grab differs
    let want = Stream {
        screen: 0,
        quality: Quality::Auto,
        fps: 15,
        cursor: false,
        h264: false,
    };
    let mut last = None;
    f.still.store(true, SeqCst);
    assert!(
        snap(&f, &f.screens(), want, &mut last).is_some(),
        "first frame"
    );
    let n = f.n.load(SeqCst);
    assert!(snap(&f, &f.screens(), want, &mut last).is_none());
    assert_eq!(f.n.load(SeqCst), n, "no grab");
    // the drawn pointer moved on a still screen: a new frame
    f.pointer.store(true, SeqCst);
    assert!(snap(&f, &f.screens(), want, &mut last).is_some());
    assert!(snap(&f, &f.screens(), want, &mut last).is_none());
    // the viewer draws the pointer: its moves need no frame
    let bare = Stream {
        cursor: true,
        ..want
    };
    assert!(
        snap(&f, &f.screens(), bare, &mut last).is_some(),
        "retarget"
    );
    f.pointer.store(false, SeqCst);
    assert!(snap(&f, &f.screens(), bare, &mut last).is_none());
    // whole desktop: only when every monitor is unchanged
    f.plugged.store(true, SeqCst);
    let all = Stream {
        screen: ALL,
        ..want
    };
    assert!(snap(&f, &f.screens(), all, &mut last).is_some());
    assert!(snap(&f, &f.screens(), all, &mut last).is_none());
    f.still.store(false, SeqCst);
    assert!(snap(&f, &f.screens(), all, &mut last).is_some());
}

#[test]
fn whole_desktop_is_every_screen_side_by_side() {
    let f = Fake::default();
    f.plugged.store(true, SeqCst);
    let screens = f.screens();
    assert!(
        target(&f.screens()[..1], ALL).is_none(),
        "one screen: no ALL"
    );
    let t = target(&screens, ALL).unwrap();
    assert_eq!((t.id, t.x, t.y, t.w, t.h), (ALL, 0, 0, 200, 50));
    let img = grab(&f, &screens, &t).unwrap();
    assert_eq!((img.width(), img.height()), (200, 50));
    // input over the whole desktop: the right edge is the second screen's
    assert_eq!(to_abs(&t, 1.0, 0.0), Some((199, 0)));
    assert_eq!(
        cur_msg(Some((150, 25)), Some("text"), &t),
        json!({"t": "cur", "x": 0.75, "y": 0.5, "shape": "text", "vis": true})
    );
    assert_eq!(cur_msg(Some((500, 25)), Some("text"), &t)["vis"], false);
    assert_eq!(cur_msg(Some((10, 10)), None, &t)["vis"], false, "hidden");
}

#[test]
fn held_keys_and_buttons_released() {
    let rec = Arc::new(Mutex::new(Vec::new()));
    let mut i = Rec(rec.clone());
    let mut h = Held::default();
    for e in [
        Ev::Key("ShiftLeft".into(), true),
        Ev::Key("KeyW".into(), true),
        Ev::Key("ShiftLeft".into(), false),
        Ev::Button(2, true),
        Ev::Button(7, true),           // unknown button: ignored
        Ev::Key("Nope".into(), true),  // unknown code: neither sent nor tracked
        Ev::Key("KeyQ".into(), false), // not held: no stray key up
    ] {
        h.apply(&mut i, e);
    }
    assert_eq!(
        *lock(&rec),
        [
            "key ShiftLeft true",
            "key KeyW true",
            "key ShiftLeft false",
            "btn 2 true"
        ]
    );
    for n in 0..40 {
        h.apply(&mut i, Ev::Key(format!("Bogus{n}"), true));
    }
    assert_eq!(h.keys, ["KeyW"], "unknown codes take no slots");
    lock(&rec).clear();
    h.release(&mut i);
    assert_eq!(*lock(&rec), ["key KeyW false", "btn 2 false"]);
    h.release(&mut i);
    assert_eq!(lock(&rec).len(), 2, "released once");
}

/// Session end: the queued backlog (a long paste) is dropped, held keys still go up, and the
/// thread ends so the caller can wait for it.
#[test]
fn input_backlog_dropped_at_session_end() {
    let fake = Arc::new(Fake::default());
    let alive = Arc::new(AtomicBool::new(true));
    let (tx, h) = spawn_input(fake.clone(), alive.clone());
    tx.send(Ev::Key("KeyW".into(), true)).unwrap();
    for _ in 0..100 {
        if !lock(&fake.rec).is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    alive.store(false, SeqCst);
    tx.send(Ev::Text("never typed".into())).unwrap();
    drop(tx);
    h.unwrap().join().unwrap();
    assert_eq!(*lock(&fake.rec), ["key KeyW true", "key KeyW false"]);
}

#[test]
fn pairing_code_lifecycle() {
    let (_d, off, _) = setup(false);
    assert!(off.new_code().is_err(), "remote off: no code");
    let (_d, r, _) = setup(true);
    assert_eq!(
        r.c.take_code("ABCD2345").unwrap_err().code,
        "no_code",
        "nothing shown yet"
    );
    let code = r.new_code().unwrap();
    assert_eq!(code.code.len(), CODE_LEN);
    assert_eq!(code.expires_in_sec, 300);
    assert!(code.code.bytes().all(|b| CODE_CHARS.contains(&b)));
    // lower case, spaces and a dash are fine; a correct code works once
    let typed = format!("{} - {}", code.code[..4].to_lowercase(), &code.code[4..]);
    assert!(r.c.take_code(&typed).is_ok());
    assert_eq!(r.c.take_code(&code.code).unwrap_err().code, "no_code");
    // five wrong tries burn it
    let code = r.new_code().unwrap().code;
    for _ in 0..CODE_TRIES {
        assert_eq!(r.c.take_code("WRONG234").unwrap_err().code, "bad_code");
    }
    assert_eq!(r.c.take_code(&code).unwrap_err().code, "no_code");
    // expiry
    let code = r.new_code().unwrap().code;
    lock(&r.c.code).as_mut().unwrap().until = Instant::now() - Duration::from_secs(1);
    assert_eq!(r.c.take_code(&code).unwrap_err().code, "no_code");
}

#[tokio::test]
async fn turning_off_drops_the_code() {
    let (_d, r, _) = setup(true);
    r.new_code().unwrap();
    lock(&r.c.settings)["remote"]["enabled"] = json!(false);
    r.sync().await;
    assert!(lock(&r.c.code).is_none());
    assert!(r.running.lock().await.is_none(), "off: never listens");
}

#[tokio::test]
async fn host_guard_wants_the_bound_address() {
    use tower::ServiceExt;
    let (_d, r, _) = setup(true);
    *lock(&r.c.bound) = Some("100.64.0.5:7813".parse().unwrap());
    let get = |host: &'static str| {
        axum::http::Request::get("/v1/remote/hello")
            .header(HOST, host)
            .body(axum::body::Body::empty())
            .unwrap()
    };
    let ok = router(r.c.clone())
        .oneshot(get("100.64.0.5:7813"))
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);
    assert_eq!(ok.headers()["x-content-type-options"], "nosniff");
    for bad in ["evil.example:7813", "100.64.0.5", "localhost:7813"] {
        let res = router(r.c.clone()).oneshot(get(bad)).await.unwrap();
        assert_eq!(res.status(), 403, "{bad}");
    }
    // Origin: RMI's webview only; none (Rust) is fine
    for (origin, want) in [
        ("http://tauri.localhost", 200),
        ("tauri://localhost", 200),
        ("http://evil.example", 403),
        ("null", 403),
    ] {
        let mut req = get("100.64.0.5:7813");
        req.headers_mut().insert(ORIGIN, origin.parse().unwrap());
        let res = router(r.c.clone()).oneshot(req).await.unwrap();
        assert_eq!(res.status(), want, "{origin}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn hello_and_pairing_over_http() {
    let (dir, r, _) = setup(true);
    let hooks = Arc::new(AtomicUsize::new(0));
    let h = hooks.clone();
    r.set_hook(Arc::new(move || {
        h.fetch_add(1, SeqCst);
    }));
    let (addr, _stop) = serve_local(&r).await;
    let v: Value = reqwest::get(format!("http://{addr}/v1/remote/hello"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        v,
        json!({"app": "rmi", "proto": 1, "name": "Test PC", "busy": false})
    );

    let (st, v) = pair_with(&addr, "AAAA2222").await;
    assert_eq!((st, v["error"]["code"].as_str()), (403, Some("no_code")));
    let token = paired_token(&r, &addr).await;
    assert!(valid_token(&token));
    let s = r.status();
    assert_eq!(s.paired.len(), 1);
    assert_eq!(s.paired[0].name, "Laptop");
    assert!(hooks.load(SeqCst) >= 2, "listening + paired");
    // stored hashed: the token itself is nowhere in the file
    let file = std::fs::read_to_string(dir.path().join(STORE_FILE)).unwrap();
    assert!(file.contains(&hash(&token)) && !file.contains(&token));
    // a restart reads it back
    let again = Remote::new(r.c.settings.clone(), dir.path(), Arc::new(Fake::default()));
    assert!(again.c.who(&token, "127.0.0.1").is_some());
    // bound to the IP it paired from
    assert!(again.c.who(&token, "127.0.0.2").is_none());
    // pairing again from the same device replaces its entry; the old token is dead
    let token2 = paired_token(&r, &addr).await;
    let s = r.status();
    assert_eq!(s.paired.len(), 1, "no duplicate");
    assert!(r.c.who(&token, "127.0.0.1").is_none());
    assert!(r.c.who(&token2, "127.0.0.1").is_some());
    // removed: the token stops working
    r.unpair(&s.paired[0].id);
    assert!(r.c.who(&token2, "127.0.0.1").is_none());
}

/// Pair once, never again: the token survives restarts from its own IP only. An entry
/// without an IP is invalid: dropped at load, never bound to whoever uses it first.
#[test]
fn pairing_survives_restart_and_no_ip_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let (token, bare) = (new_token(), new_token());
    let at = "2026-01-01T00:00:00Z";
    let file = json!({"paired": [
        {"hash": hash(&token), "name": "Laptop", "pairedAt": at, "ip": "100.64.0.7"},
        {"hash": hash(&bare), "name": "Old", "pairedAt": at},
    ], "log": []});
    std::fs::write(dir.path().join(STORE_FILE), file.to_string()).unwrap();
    let s = Arc::new(Mutex::new(json!({"remote": {"enabled": true}})));
    let load = || Remote::new(s.clone(), dir.path(), Arc::new(Fake::default()));
    for _ in 0..2 {
        let r = load(); // app restart
        assert_eq!(
            r.c.who(&token, "100.64.0.7"),
            Some(("Laptop".into(), hash(&token)[..12].to_owned()))
        );
        assert!(r.c.who(&token, "100.64.0.8").is_none());
        assert!(r.c.who(&bare, "100.64.0.9").is_none(), "no IP: never bound");
        assert_eq!(r.status().paired.len(), 1);
    }
}

#[test]
fn unreadable_store_is_set_aside() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(STORE_FILE);
    std::fs::write(&file, b"{ not json").unwrap();
    let r = Remote::new(
        Arc::new(Mutex::new(json!({}))),
        dir.path(),
        Arc::new(Fake::default()),
    );
    assert!(r.status().paired.is_empty());
    assert!(!file.exists());
    assert_eq!(
        std::fs::read(dir.path().join("remote.json.bad")).unwrap(),
        b"{ not json"
    );
}

#[test]
fn linux_never_hosts() {
    let dir = tempfile::tempdir().unwrap();
    let s = json!({"remote": {"enabled": true}});
    let r = Remote::new(Arc::new(Mutex::new(s)), dir.path(), Arc::new(desk::Null));
    assert!(!r.enabled());
    assert!(r.new_code().is_err());
}

#[test]
fn burned_or_used_code_is_not_live() {
    let (_d, r, _) = setup(true);
    let hooks = Arc::new(AtomicUsize::new(0));
    let h = hooks.clone();
    r.set_hook(Arc::new(move || {
        h.fetch_add(1, SeqCst);
    }));
    assert!(!r.status().code_live);
    r.new_code().unwrap();
    assert!(r.status().code_live);
    for _ in 0..CODE_TRIES {
        let _ = r.c.take_code("WRONG234");
    }
    assert!(!r.status().code_live);
    assert_eq!(hooks.load(SeqCst), 1, "Settings told to hide it");
    let code = r.new_code().unwrap().code;
    r.c.take_code(&code).unwrap();
    assert!(!r.status().code_live);
}

#[tokio::test(flavor = "multi_thread")]
async fn pairing_refused_when_it_cannot_be_saved() {
    let (dir, r, _) = setup(true);
    std::fs::create_dir(dir.path().join(STORE_FILE)).unwrap(); // unwritable
    let (addr, _stop) = serve_local(&r).await;
    let code = r.new_code().unwrap().code;
    let (st, v) = pair_with(&addr, &code).await;
    assert_eq!(st, 500, "{v}");
    assert!(v.get("token").is_none());
    assert!(r.status().paired.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn pre_auth_sockets_capped() {
    let (_d, r, _) = setup(true);
    let (addr, _stop) = serve_local(&r).await;
    let mut held = Vec::new();
    for _ in 0..MAX_PRE_AUTH {
        let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/v1/remote/ws"))
            .await
            .unwrap();
        held.push(ws);
    }
    assert!(
        tokio_tungstenite::connect_async(format!("ws://{addr}/v1/remote/ws"))
            .await
            .is_err(),
        "one too many"
    );
    held.clear();
    for _ in 0..100 {
        if r.c.pre_auth.available_permits() == MAX_PRE_AUTH {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        r.c.pre_auth.available_permits(),
        MAX_PRE_AUTH,
        "freed on drop"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_source_ip_refused() {
    let (_d, r, _) = setup(true);
    let (addr, _stop) = serve_local(&r).await;
    let token = paired_token(&r, &addr).await;
    lock(&r.c.store).paired[0].ip = "100.64.0.99".into();
    let mut ws = open(&addr, &auth(&token)).await;
    assert_eq!(
        next_ctl(&mut ws).await,
        json!({"t": "bye", "code": "auth", "why": "This computer no longer knows this device. Pair again."})
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn silent_viewer_times_out_and_releases() {
    let (_d, r, fake) = setup(true);
    let (addr, _stop) = serve_local(&r).await;
    let token = paired_token(&r, &addr).await;
    let mut ws = open(&addr, &auth(&token)).await;
    assert_eq!(next_ctl(&mut ws).await["t"], "hello");
    send(&mut ws, json!({"t": "kd", "code": "KeyW"})).await;
    // then nothing: no ping, socket left open
    let deadline = Instant::now() + IDLE + Duration::from_secs(3);
    let bye = loop {
        let v = next_ctl(&mut ws).await;
        if v["t"] == "bye" || Instant::now() > deadline {
            break v;
        }
    };
    assert_eq!(bye["code"], "timeout");
    assert!(r.live().is_none(), "free for a reconnect");
    for _ in 0..100 {
        if lock(&fake.rec).last().map(String::as_str) == Some("key KeyW false") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(*lock(&fake.rec), ["key KeyW true", "key KeyW false"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn releases_pass_in_view_only_and_screen_change_keeps_session() {
    let (_d, r, fake) = setup(true);
    let (addr, _stop) = serve_local(&r).await;
    let token = paired_token(&r, &addr).await;
    let mut ws = open(&addr, &auth(&token)).await;
    let hello = next_ctl(&mut ws).await;
    assert_eq!(hello["combos"], json!(["lock"]));
    assert_eq!(hello["screen"], 0);
    assert_eq!(
        hello["caps"],
        with_h264(&["files", "audio"]),
        "all even with one screen: a second may be plugged in"
    );
    send(&mut ws, json!({"t": "kd", "code": "KeyW"})).await;
    send(&mut ws, json!({"t": "md", "b": 2, "x": 0, "y": 0})).await;
    send(&mut ws, json!({"t": "view", "on": true})).await;
    send(&mut ws, json!({"t": "ku", "code": "KeyW"})).await;
    send(&mut ws, json!({"t": "mu", "b": 2, "x": 1, "y": 1})).await;
    send(&mut ws, json!({"t": "ping", "n": 1})).await;
    while next_ctl(&mut ws).await != json!({"t": "pong", "n": 1}) {}
    // a monitor plugged in: the new list, same session
    fake.plugged.store(true, SeqCst);
    let v = loop {
        let v = next_ctl(&mut ws).await;
        if v["t"] == "screens" {
            break v;
        }
    };
    assert_eq!(v["screen"], 0);
    assert_eq!(
        v["screens"][1],
        json!({"id": 1, "w": 100, "h": 50, "x": 100, "y": 0, "primary": false})
    );
    // the whole desktop now streams and takes input across both
    send(&mut ws, json!({"t": "view", "on": false})).await;
    send(&mut ws, json!({"t": "start", "screen": 255})).await;
    let f = loop {
        let f = next(&mut ws, 3000).await.unwrap();
        if f[0] == 0x01 {
            break f;
        }
    };
    assert_eq!(f[1], 255);
    assert_eq!(image::load_from_memory(&f[2..]).unwrap().width(), 200);
    send(&mut ws, json!({"t": "mm", "x": 1, "y": 0})).await;
    send(&mut ws, json!({"t": "ping", "n": 2})).await;
    while next_ctl(&mut ws).await != json!({"t": "pong", "n": 2}) {}
    ws.close(None).await.unwrap();
    wait_free(&r).await;
    for _ in 0..100 {
        if lock(&fake.rec).len() >= 6 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        *lock(&fake.rec),
        [
            "key KeyW true",
            "move 0,0",
            "btn 2 true",
            "key KeyW false",
            "btn 2 false",
            "move 199,0"
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn evicted_device_loses_its_session() {
    let (_d, r, _) = setup(true);
    let (addr, _stop) = serve_local(&r).await;
    let token = paired_token(&r, &addr).await;
    let mut ws = open(&addr, &auth(&token)).await;
    assert_eq!(next_ctl(&mut ws).await["t"], "hello");
    {
        let mut st = lock(&r.c.store);
        st.paired[0].ip = "127.0.0.2".into(); // not replaced as the same device below
        for n in 1..MAX_PAIRED {
            st.paired.push(Paired {
                hash: hash(&n.to_string()),
                name: "x".into(),
                paired_at: now_iso(),
                ip: "100.64.0.1".into(),
            });
        }
    }
    paired_token(&r, &addr).await; // evicts the oldest: the live one
    assert_eq!(bye_of(&mut ws).await["code"], "removed");
}

#[tokio::test(flavor = "multi_thread")]
async fn session_streams_with_backpressure_and_releases_input() {
    let (_d, r, fake) = setup(true);
    let (addr, _stop) = serve_local(&r).await;
    let token = paired_token(&r, &addr).await;

    // wrong token: bye, no session
    let mut bad = open(&addr, &auth(&"0".repeat(64))).await;
    assert_eq!(next_ctl(&mut bad).await["code"], "auth");

    let mut ws = open(&addr, &auth(&token)).await;
    let hello = next_ctl(&mut ws).await;
    assert_eq!(hello["t"], "hello");
    assert_eq!(hello["host"], "Test PC");
    assert_eq!(
        hello["screens"],
        json!([{"id": 0, "w": 100, "h": 50, "x": 0, "y": 0, "primary": true}])
    );
    assert_eq!(hello["canInput"], true);
    assert_eq!(r.live().unwrap().peer, "Laptop");

    // one session at a time: another device is told who has it
    let other = other_device(&r, "Den PC");
    let mut second = open(&addr, &auth(&other)).await;
    let bye = next_ctl(&mut second).await;
    assert_eq!(
        (&bye["code"], &bye["by"]),
        (&json!("busy"), &json!("Laptop"))
    );

    send(&mut ws, json!({"t": "ping", "n": 7})).await;
    assert_eq!(next_ctl(&mut ws).await, json!({"t": "pong", "n": 7}));

    // at most MAX_UNACKED frames without an ack, then one more per ack
    send(&mut ws, json!({"t": "start", "screen": 0, "maxFps": 60})).await;
    for _ in 0..MAX_UNACKED {
        let f = next_frame(&mut ws, 3000).await.unwrap();
        assert_eq!(&f[..2], &[0x01, 0]);
    }
    assert!(
        next_frame(&mut ws, 500).await.is_none(),
        "one frame too many before any ack"
    );
    send(&mut ws, json!({"t": "ack"})).await;
    assert_eq!(next_frame(&mut ws, 3000).await.unwrap()[0], 0x01);
    // stats every 2 s while streaming
    let stats = loop {
        let v = next_ctl(&mut ws).await;
        if v["t"] == "stats" {
            break v;
        }
    };
    // frames still unacknowledged: auto may have stepped down from 70 already
    assert!(matches!(stats["q"].as_u64(), Some(60 | 70)), "{stats}");
    assert!(stats["fps"].is_number() && stats["kbps"].is_number());
    assert!(
        stats["inflight"].is_number() && stats["w"].is_number(),
        "{stats}"
    );

    // view only: input ignored
    send(&mut ws, json!({"t": "view", "on": true})).await;
    send(&mut ws, json!({"t": "kd", "code": "KeyQ"})).await;
    send(&mut ws, json!({"t": "view", "on": false})).await;
    send(&mut ws, json!({"t": "md", "b": 0, "x": 0.5, "y": 0.5})).await;
    send(&mut ws, json!({"t": "kd", "code": "KeyW"})).await;
    // a paste arrives as several text chunks: typed in order
    send(&mut ws, json!({"t": "text", "s": "hi"})).await;
    send(&mut ws, json!({"t": "text", "s": " there"})).await;
    // up to 8 KB per message (spec)
    send(&mut ws, json!({"t": "text", "s": "x".repeat(8000)})).await;
    send(&mut ws, json!({"t": "kd", "code": "AudioVolumeUp"})).await;
    send(&mut ws, json!({"t": "ku", "code": "AudioVolumeUp"})).await;
    send(&mut ws, json!({"t": "combo", "name": "cad"})).await;
    send(&mut ws, json!({"t": "nonsense"})).await;
    send(&mut ws, json!({"t": "ping", "n": 8})).await; // ordering barrier
    loop {
        if next_ctl(&mut ws).await == json!({"t": "pong", "n": 8}) {
            break;
        }
    }
    ws.close(None).await.unwrap();

    // input thread: in order, then everything held is released
    let long = format!("text {}", "x".repeat(8000));
    let want = [
        "move 49,24",
        "btn 0 true",
        "key KeyW true",
        "text hi",
        "text  there",
        &long,
        "key AudioVolumeUp true",
        "key AudioVolumeUp false",
        "key KeyW false",
        "btn 0 false",
    ];
    for _ in 0..100 {
        if lock(&fake.rec).len() >= want.len() && r.live().is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(*lock(&fake.rec), want);
    let log = r.status().log;
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].peer, "Laptop");
    assert!(log[0].control);
}

#[tokio::test(flavor = "multi_thread")]
async fn reconnect_and_takeover_replace_the_session() {
    let (_d, r, fake) = setup(true);
    let (addr, _stop) = serve_local(&r).await;
    let token = paired_token(&r, &addr).await;
    let busy = || async {
        let v: Value = reqwest::get(format!("http://{addr}/v1/remote/hello"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        v["busy"].as_bool().unwrap()
    };
    let mut stale = open(&addr, &auth(&token)).await;
    assert_eq!(next_ctl(&mut stale).await["t"], "hello");
    assert!(busy().await, "Devices shows it in use");
    // the same device again (its old socket died unseen): replaces, never busy
    let mut ws = open(&addr, &auth(&token)).await;
    assert_eq!(next_ctl(&mut ws).await["t"], "hello");
    assert_eq!(bye_of(&mut stale).await["code"], "replaced");
    // pointer as `cur`, left out of frames
    fake.pointer.store(true, SeqCst);
    send(&mut ws, json!({"t": "start", "cursor": true})).await;
    let cur = loop {
        let v = next_ctl(&mut ws).await;
        if v["t"] == "cur" {
            break v;
        }
    };
    assert_eq!(
        cur,
        json!({"t": "cur", "x": 0.1, "y": 0.4, "shape": "text", "vis": true})
    );
    // another device asking to take over
    let other = other_device(&r, "Den PC");
    let first = json!({"t": "auth", "token": other, "takeover": true}).to_string();
    let mut taker = open(&addr, &first).await;
    assert_eq!(next_ctl(&mut taker).await["t"], "hello");
    assert_eq!(bye_of(&mut ws).await["code"], "replaced");
    assert_eq!(r.live().unwrap().peer, "Den PC");
}

#[tokio::test(flavor = "multi_thread")]
async fn off_and_stop_end_sessions() {
    let (_d, r, _) = setup(true);
    let (addr, _stop) = serve_local(&r).await;
    let token = paired_token(&r, &addr).await;

    // Stop with nothing live (a replacement between sessions) still holds new ones off
    r.stop_session();
    let mut early = open(&addr, &auth(&token)).await;
    assert_eq!(next_ctl(&mut early).await["code"], "stopped");
    *lock(&r.c.stopped_at) = None;

    // Stop button: bye stopped, logged as view only
    let mut ws = open(&addr, &auth(&token)).await;
    assert_eq!(next_ctl(&mut ws).await["t"], "hello");
    r.stop_session();
    assert_eq!(
        bye_of(&mut ws).await,
        json!({"t": "bye", "code": "stopped", "why": "Stopped at the device."})
    );
    wait_free(&r).await;
    assert!(!r.status().log[0].control);
    // refused for 30 s after Stop, takeover or not
    for first in [
        auth(&token),
        json!({"t": "auth", "token": token, "takeover": true}).to_string(),
    ] {
        let mut again = open(&addr, &first).await;
        assert_eq!(next_ctl(&mut again).await["code"], "stopped");
    }
    *lock(&r.c.stopped_at) = Some(Instant::now() - STOP_HOLD);
    let mut again = open(&addr, &auth(&token)).await;
    assert_eq!(next_ctl(&mut again).await["t"], "hello", "after 30 s");
    drop(again);
    wait_free(&r).await;

    // switched off: a new socket gets state off + bye, even with a good token
    lock(&r.c.settings)["remote"]["enabled"] = json!(false);
    let mut ws = open(&addr, &auth(&token)).await;
    assert_eq!(
        next_ctl(&mut ws).await,
        json!({"t": "state", "state": "off", "canInput": false})
    );
    assert_eq!(next_ctl(&mut ws).await["code"], "off");
    let (st, v) = pair_with(&addr, "AAAA2222").await;
    assert_eq!((st, v["error"]["code"].as_str()), (403, Some("off")));
}

// ---- files and sound ----------------------------------------------------------------------

/// Real WASAPI loopback for 2 s (read only: captures, plays nothing). Manual:
/// `cargo test --lib real_loopback -- --ignored --nocapture`
#[cfg(windows)]
#[test]
#[ignore = "touches the real sound device"]
fn real_loopback_capture() {
    let until = Instant::now() + Duration::from_secs(2);
    let (mut fmt, mut samples) = (None, 0usize);
    audio::loopback(&|| Instant::now() < until, &mut |s| match s {
        audio::Sound::Format(r, c) => fmt = Some((r, c)),
        audio::Sound::Pcm(p) => samples += p.len(),
        audio::Sound::Fail(why) => println!("{why}"),
    });
    println!("format {fmt:?}, {samples} samples in 2 s (0 when nothing plays)");
    assert!(fmt.is_some(), "no default output device or capture failed");
}

#[test]
fn upload_names_are_sanitised() {
    use files::safe_name;
    for (raw, want) in [
        ("b.zip", "b.zip"),
        ("../../evil/b.zip", "b.zip"),
        ("C:\\Windows\\System32\\x.dll", "x.dll"),
        ("a<b>c:d\"e|f?g*h.txt", "abcdefgh.txt"),
        ("CON", "_CON"),
        ("con.txt", "_con.txt"),
        ("Lpt3.tar.gz", "_Lpt3.tar.gz"),
        ("COM¹.txt", "_COM¹.txt"),
        ("console.txt", "console.txt"),
        ("name. . .", "name"),
        ("  spaced  ", "spaced"),
        ("..", "file"),
        ("", "file"),
        ("dir/", "file"),
        ("tab\there.txt", "tabhere.txt"),
        (".bashrc", ".bashrc"),
        ("x.txt:hidden", "x.txthidden"),
    ] {
        assert_eq!(safe_name(raw), want, "{raw:?}");
    }
    let long = format!("{}.pdf", "a".repeat(400));
    let s = safe_name(&long);
    assert_eq!(s.chars().count(), 150);
    assert!(s.ends_with("a.pdf"), "the extension survives: {s}");
    // the cap is in bytes: 3-byte CJK and 4-byte emoji names still fit 255-byte file systems
    for c in ["漢", "😀"] {
        let s = safe_name(&format!("{}.pdf", c.repeat(100)));
        assert!(s.len() <= 150 && s.len() > 140, "{} bytes", s.len());
        assert!(
            s.ends_with(&format!("{c}.pdf")),
            "cut on a char boundary: {s}"
        );
    }
}

#[test]
fn names_drop_bidi_and_zero_width_characters() {
    assert_eq!(clean_name("  Den\u{7} PC  "), "Den PC");
    assert_eq!(clean_name("evil\u{202e}gpj.exe"), "evilgpj.exe");
    assert_eq!(
        clean_name("\u{feff}a\u{200b}b\u{2066}c\u{2069}\u{200f}"),
        "abc"
    );
    // ordinary letters of every script stay
    assert_eq!(clean_name("Комп 東京 حاسوب"), "Комп 東京 حاسوب");
    assert_eq!(clean_name("a\u{200e}b"), "ab");
    // joiners stay: Persian words and joined emoji need them
    assert_eq!(clean_name("می\u{200c}خواهم"), "می\u{200c}خواهم");
    assert_eq!(clean_name("👩\u{200d}💻"), "👩\u{200d}💻");
}

#[test]
fn uploads_never_overwrite() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("b.zip"), "old").unwrap();
    std::fs::write(d.path().join("b (1).zip"), "old").unwrap();
    let (_, p) = files::create_unique(d.path(), "b.zip", false).unwrap();
    assert_eq!(p, d.path().join("b (2).zip"));
    let (_, p) = files::create_unique(d.path(), "noext", false).unwrap();
    assert_eq!(p, d.path().join("noext"));
    let (_, p) = files::create_unique(d.path(), "noext", false).unwrap();
    assert_eq!(p, d.path().join("noext (1)"));
    assert_eq!(std::fs::read(d.path().join("b.zip")).unwrap(), b"old");
    // a partial upload: `<free name>.part`, the name itself also free
    std::fs::write(d.path().join("b (4).zip.part"), "old").unwrap();
    let (_, p) = files::create_unique(d.path(), "b.zip", true).unwrap();
    assert_eq!(p, d.path().join("b (3).zip"));
    assert!(d.path().join("b (3).zip.part").exists() && !p.exists());
    let (_, p) = files::create_unique(d.path(), "b.zip", true).unwrap();
    assert_eq!(p, d.path().join("b (5).zip"));
    assert_eq!(
        std::fs::read(d.path().join("b (4).zip.part")).unwrap(),
        b"old"
    );
}

#[test]
fn network_and_device_paths_are_refused() {
    use std::path::Path;
    assert!(files::local_abs(std::env::temp_dir().as_path()));
    assert!(!files::local_abs(Path::new("relative/x")));
    #[cfg(windows)]
    for p in [
        r"\\host\share\x",
        "//host/share/x",
        r"\\?\UNC\host\share\x",
        r"\\.\pipe\x",
        r"\\.\C:\x",
        r"\\?\GLOBALROOT\Device\Mup\host\share\x",
        r"\\?\GLOBALROOT\Device\NamedPipe\x",
        r"\\?\Volume{00000000-0000-0000-0000-000000000000}\x",
        r"\??\UNC\host\share\x",
        r"C:x",
    ] {
        assert!(!files::local_abs(Path::new(p)), "{p}");
    }
    #[cfg(windows)]
    for p in [r"C:\Users\x", "C:/Users/x", r"\\?\C:\Users\x"] {
        assert!(files::local_abs(Path::new(p)), "{p}");
    }
    #[cfg(not(windows))]
    assert!(files::local_abs(Path::new("/home/x")));
}

#[test]
fn listing_puts_folders_first() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("b.txt"), "12345").unwrap();
    std::fs::write(d.path().join("A.txt"), "").unwrap();
    std::fs::create_dir(d.path().join("zdir")).unwrap();
    let v = files::list(&d.path().to_string_lossy());
    let names: Vec<_> = v["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["name"].as_str().unwrap(),
                e["dir"] == true,
                e["size"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        names,
        [("zdir", true, 0), ("A.txt", false, 0), ("b.txt", false, 5)]
    );
    assert!(v["entries"][0]["mtime"].as_u64().unwrap() > 1_600_000_000);
    assert_eq!(
        v["parent"],
        json!(d.path().parent().unwrap().to_string_lossy())
    );
    let gone = files::list(&d.path().join("nope").to_string_lossy());
    assert_eq!(gone["err"], "Not found.");
    assert_eq!(files::list("relative")["err"], "Use a full path.");
    assert_eq!(
        files::list("")["path"],
        json!(dirs::home_dir().unwrap().to_string_lossy())
    );
}

#[test]
fn pcm_conversion_and_sound_never_queues() {
    use audio::{f32_to_i16, i32_to_i16};
    assert_eq!(
        [1.0, -1.0, 0.0, 0.5, 2.0, -7.0, f32::NAN].map(f32_to_i16),
        [32767, -32767, 0, 16384, 32767, -32767, 0]
    );
    assert_eq!(
        [i32::MAX, i32::MIN, 0, 1 << 16].map(i32_to_i16),
        [32767, -32768, 0, 1]
    );
    let q = audio::AudioQ::default();
    for i in 0..15u8 {
        q.push(Bytes::from(vec![i]));
    }
    // 200 ms (10 messages) at most: the oldest went
    let left: Vec<u8> = std::iter::from_fn(|| q.pop()).map(|b| b[0]).collect();
    assert_eq!(left, (5..15).collect::<Vec<_>>());
}

/// The next control message with this `t`.
async fn ctl_t(ws: &mut Ws, t: &str) -> Value {
    loop {
        let v = next_ctl(ws).await;
        if v["t"] == t {
            return v;
        }
    }
}

fn up_chunk(id: u32, data: &[u8]) -> WsMsg {
    let mut m = vec![0x10];
    m.extend(id.to_be_bytes());
    m.extend(data);
    WsMsg::binary(m)
}

/// Sends a ping and waits for its pong: everything sent before was handled.
async fn barrier(ws: &mut Ws, n: u64) {
    send(ws, json!({"t": "ping", "n": n})).await;
    while next_ctl(ws).await != json!({"t": "pong", "n": n}) {}
}

async fn until(what: &str, f: impl Fn() -> bool) {
    for _ in 0..150 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for: {what}");
}

/// A live session whose uploads land in `<tmp>/dl`.
async fn file_session(
    settings: Value,
) -> (tempfile::TempDir, Remote, Ws, PathBuf, oneshot::Sender<()>) {
    let (d, r, _) = setup(true);
    for (k, v) in settings.as_object().unwrap() {
        lock(&r.c.settings)["remote"][k] = v.clone();
    }
    let dl = d.path().join("dl");
    *lock(&r.c.downloads) = Some(dl.clone());
    let (addr, stop) = serve_local(&r).await;
    let token = paired_token(&r, &addr).await;
    let mut ws = open(&addr, &auth(&token)).await;
    assert_eq!(next_ctl(&mut ws).await["t"], "hello");
    (d, r, ws, dl, stop)
}

#[tokio::test(flavor = "multi_thread")]
async fn files_round_trip_with_flow_control() {
    let (d, r, mut ws, dl, _stop) = file_session(json!({})).await;
    // download: 5 MB + a bit, at most 4 MB before an ack
    let src = d.path().join("big.bin");
    let body: Vec<u8> = (0..5 * 1024 * 1024 + 123)
        .map(|i| (i % 251) as u8)
        .collect();
    std::fs::write(&src, &body).unwrap();
    send(
        &mut ws,
        json!({"t": "file", "op": "get", "id": 7, "path": src}),
    )
    .await;
    assert_eq!(
        ctl_t(&mut ws, "file").await,
        json!({"t": "file", "id": 7, "name": "big.bin", "size": body.len()})
    );
    let mut got: Vec<u8> = Vec::new();
    while let Some(b) = next(&mut ws, 500).await {
        if b[0] == 0x04 {
            assert_eq!(&b[1..5], &7u32.to_be_bytes());
            assert!(b.len() - 5 <= files::CHUNK);
            got.extend(&b[5..]);
        }
    }
    assert_eq!(got.len(), 4 * 1024 * 1024, "stops at the 4 MB window");
    send(
        &mut ws,
        json!({"t": "file", "op": "ack", "id": 7, "got": got.len()}),
    )
    .await;
    loop {
        let b = next(&mut ws, 3000).await.expect("rest of the download");
        match b[0] {
            0x04 => got.extend(&b[5..]),
            0x02 => {
                let v: Value = serde_json::from_slice(&b[1..]).unwrap();
                if v["t"] == "file" {
                    assert_eq!(v, json!({"t": "file", "id": 7, "done": true}));
                    break;
                }
            }
            _ => {}
        }
    }
    assert!(got == body, "download intact");

    // upload: a path in the name is dropped, an existing file is kept
    std::fs::create_dir_all(&dl).unwrap();
    std::fs::write(dl.join("b.zip"), "old").unwrap();
    let up: Vec<u8> = (0..1536 * 1024 + 7).map(|i| (i % 13) as u8).collect();
    let name = "../../evil/b.zip";
    send(
        &mut ws,
        json!({"t": "file", "op": "put", "id": 8, "name": name, "size": up.len()}),
    )
    .await;
    for c in up.chunks(files::CHUNK) {
        ws.send(up_chunk(8, c)).await.unwrap();
    }
    send(&mut ws, json!({"t": "file", "op": "end", "id": 8})).await;
    let mut gots = Vec::new();
    let done = loop {
        let v = ctl_t(&mut ws, "file").await;
        if v.get("got").is_some() {
            gots.push(v["got"].as_u64().unwrap());
        } else {
            break v;
        }
    };
    assert!(gots.contains(&(1024 * 1024)), "got every 1 MB: {gots:?}");
    let saved = dl.join("b (1).zip");
    assert_eq!(
        done,
        json!({"t": "file", "id": 8, "done": true, "path": saved.to_string_lossy()})
    );
    assert!(std::fs::read(&saved).unwrap() == up, "upload intact");
    assert!(!dl.join("b (1).zip.part").exists(), "renamed from .part");
    assert_eq!(std::fs::read(dl.join("b.zip")).unwrap(), b"old");
    assert!(!d.path().join("evil").exists());

    ws.close(None).await.unwrap();
    wait_free(&r).await;
    use files::{Dir, FileLog};
    assert_eq!(
        r.status().log[0].files,
        [
            FileLog {
                name: "big.bin".into(),
                size: body.len() as u64,
                dir: Dir::Sent
            },
            FileLog {
                name: "b (1).zip".into(),
                size: up.len() as u64,
                dir: Dir::Received
            },
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_and_disconnect_delete_partial_uploads() {
    let (_d, r, mut ws, dl, _stop) = file_session(json!({})).await;
    send(
        &mut ws,
        json!({"t": "file", "op": "put", "id": 1, "name": "a.txt", "size": 1000}),
    )
    .await;
    ws.send(up_chunk(1, b"0123456789")).await.unwrap();
    barrier(&mut ws, 1).await;
    // written as .part, never under the real name
    until("partial a.txt.part", || dl.join("a.txt.part").exists()).await;
    assert!(!dl.join("a.txt").exists());
    send(&mut ws, json!({"t": "file", "op": "cancel", "id": 1})).await;
    until("a.txt.part deleted on cancel", || {
        !dl.join("a.txt.part").exists()
    })
    .await;

    send(
        &mut ws,
        json!({"t": "file", "op": "put", "id": 2, "name": "c.txt", "size": 1000}),
    )
    .await;
    ws.send(up_chunk(2, b"0123456789")).await.unwrap();
    barrier(&mut ws, 2).await;
    until("partial c.txt.part", || dl.join("c.txt.part").exists()).await;
    drop(ws);
    until("c.txt.part deleted on disconnect", || {
        !dl.join("c.txt.part").exists()
    })
    .await;
    assert!(
        std::fs::read_dir(&dl).unwrap().next().is_none(),
        "nothing left"
    );
    wait_free(&r).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn transfers_refused_in_view_only_and_over_limits() {
    let (d, _r, mut ws, dl, _stop) = file_session(json!({})).await;
    let err = |id: u32, e: &str| json!({"t": "file", "id": id, "err": e});
    send(&mut ws, json!({"t": "view", "on": true})).await;
    let view = "View only is on. Switch it off to transfer files.";
    send(&mut ws, json!({"t": "fs", "op": "list", "path": ""})).await;
    assert_eq!(
        ctl_t(&mut ws, "fs").await,
        json!({"t": "fs", "path": "", "err": view})
    );
    send(
        &mut ws,
        json!({"t": "file", "op": "put", "id": 1, "name": "a", "size": 1}),
    )
    .await;
    assert_eq!(ctl_t(&mut ws, "file").await, err(1, view));
    send(&mut ws, json!({"t": "view", "on": false})).await;

    let big = files::MAX_SIZE + 1;
    send(
        &mut ws,
        json!({"t": "file", "op": "put", "id": 1, "name": "a", "size": big}),
    )
    .await;
    assert_eq!(
        ctl_t(&mut ws, "file").await,
        err(1, "Files over 4 GB cannot be sent.")
    );
    send(
        &mut ws,
        json!({"t": "file", "op": "get", "id": 1, "path": "rel.txt"}),
    )
    .await;
    assert_eq!(ctl_t(&mut ws, "file").await, err(1, "Use a full path."));
    send(
        &mut ws,
        json!({"t": "file", "op": "get", "id": 1, "path": d.path()}),
    )
    .await;
    assert_eq!(ctl_t(&mut ws, "file").await, err(1, "That is not a file."));

    // two at a time
    send(
        &mut ws,
        json!({"t": "file", "op": "put", "id": 1, "name": "x", "size": 10}),
    )
    .await;
    send(
        &mut ws,
        json!({"t": "file", "op": "put", "id": 2, "name": "y", "size": 3}),
    )
    .await;
    send(
        &mut ws,
        json!({"t": "file", "op": "put", "id": 1, "name": "z", "size": 1}),
    )
    .await;
    assert_eq!(
        ctl_t(&mut ws, "file").await,
        err(1, "That transfer id is in use.")
    );
    send(
        &mut ws,
        json!({"t": "file", "op": "put", "id": 3, "name": "z", "size": 1}),
    )
    .await;
    assert_eq!(
        ctl_t(&mut ws, "file").await,
        err(
            3,
            "Two transfers are running already. Wait for one to finish."
        )
    );
    // short and long uploads fail and are deleted
    send(&mut ws, json!({"t": "file", "op": "end", "id": 1})).await;
    assert_eq!(
        ctl_t(&mut ws, "file").await,
        err(1, "The file arrived incomplete.")
    );
    ws.send(up_chunk(2, b"12345")).await.unwrap();
    assert_eq!(
        ctl_t(&mut ws, "file").await,
        err(2, "More data arrived than the file's size.")
    );
    until("failed uploads deleted", || {
        std::fs::read_dir(&dl).is_ok_and(|mut d| d.next().is_none())
    })
    .await;

    // a binary message over the limit ends the session
    ws.send(up_chunk(9, &vec![0; files::CHUNK + 1]))
        .await
        .unwrap();
    let mut closed = false;
    for _ in 0..50 {
        if next(&mut ws, 100).await.is_none() {
            closed = true;
            break;
        }
    }
    assert!(closed);
}

#[tokio::test(flavor = "multi_thread")]
async fn binary_or_oversized_first_message_is_no_auth() {
    let (_d, r, _) = setup(true);
    let (addr, _stop) = serve_local(&r).await;
    let token = paired_token(&r, &addr).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/v1/remote/ws"))
        .await
        .unwrap();
    ws.send(WsMsg::binary(auth(&token).into_bytes()))
        .await
        .unwrap();
    assert_eq!(next_ctl(&mut ws).await["code"], "auth");
    let padded = json!({"t": "auth", "token": token, "pad": "x".repeat(MAX_MSG)}).to_string();
    let mut ws = open(&addr, &padded).await;
    assert_eq!(next_ctl(&mut ws).await["code"], "auth");
    assert!(r.live().is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn switched_off_files_and_sound_drop_their_caps() {
    let (_d, r, _) = setup(true);
    {
        let mut s = lock(&r.c.settings);
        s["remote"]["files"] = json!(false);
        s["remote"]["audio"] = json!(false);
    }
    let (addr, _stop) = serve_local(&r).await;
    let token = paired_token(&r, &addr).await;
    let mut ws = open(&addr, &auth(&token)).await;
    let hello = next_ctl(&mut ws).await;
    assert_eq!(hello["caps"], with_h264(&[]));
    send(&mut ws, json!({"t": "fs", "op": "list", "path": ""})).await;
    assert_eq!(
        ctl_t(&mut ws, "fs").await["err"],
        "File transfer is switched off on that computer."
    );
    send(&mut ws, json!({"t": "audio", "on": true})).await;
    barrier(&mut ws, 1).await;
    while let Some(b) = next(&mut ws, 300).await {
        assert_ne!(b[0], 0x05, "no sound");
        if b[0] == 0x02 {
            let v: Value = serde_json::from_slice(&b[1..]).unwrap();
            assert_ne!(v["t"], "audio");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn sound_streams_in_20ms_messages_and_stops() {
    let (_d, r, mut ws, _dl, _stop) = file_session(json!({})).await;
    send(&mut ws, json!({"t": "audio", "on": true})).await;
    assert_eq!(
        ctl_t(&mut ws, "audio").await,
        json!({"t": "audio", "rate": 48000, "ch": 2})
    );
    let mut n = 0;
    while n < 5 {
        let b = next(&mut ws, 2000).await.expect("sound");
        if b[0] == 0x05 {
            assert_eq!(b.len(), 1 + 960 * 2 * 2, "20 ms of 48 kHz stereo s16");
            assert_eq!(&b[1..3], &1i16.to_le_bytes());
            n += 1;
        }
    }
    send(&mut ws, json!({"t": "audio", "on": false})).await;
    barrier(&mut ws, 1).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    while next(&mut ws, 100).await.is_some() {} // what was already queued
    send(&mut ws, json!({"t": "ping", "n": 2})).await;
    while let Some(b) = next(&mut ws, 300).await {
        assert_ne!(b[0], 0x05, "stopped");
    }
    drop(ws);
    wait_free(&r).await;
}

/// A live session on `r`, past `hello`.
async fn live_ws(r: &Remote) -> (Ws, oneshot::Sender<()>) {
    let (addr, stop) = serve_local(r).await;
    let token = paired_token(r, &addr).await;
    let mut ws = open(&addr, &auth(&token)).await;
    assert_eq!(next_ctl(&mut ws).await["t"], "hello");
    (ws, stop)
}

#[tokio::test(flavor = "multi_thread")]
async fn surround_sound_is_sent_as_stereo() {
    let (_d, r, fake) = setup(true);
    fake.surround.store(true, SeqCst);
    let (mut ws, _stop) = live_ws(&r).await;
    send(&mut ws, json!({"t": "audio", "on": true})).await;
    assert_eq!(
        ctl_t(&mut ws, "audio").await,
        json!({"t": "audio", "rate": 48000, "ch": 2})
    );
    loop {
        let b = next(&mut ws, 2000).await.expect("sound");
        if b[0] == 0x05 {
            assert_eq!(b.len(), 1 + 960 * 2 * 2, "20 ms of 48 kHz stereo s16");
            let s: Vec<i16> = b[1..9]
                .chunks(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]))
                .collect();
            assert_eq!(s, [1, 2, 1, 2], "first two channels of each frame");
            break;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_sound_capture_says_why_and_a_new_request_retries() {
    let (_d, r, fake) = setup(true);
    fake.no_sound.store(true, SeqCst);
    let (mut ws, _stop) = live_ws(&r).await;
    send(&mut ws, json!({"t": "audio", "on": true})).await;
    assert_eq!(
        ctl_t(&mut ws, "audio").await,
        json!({"t": "audio", "on": false, "err": "No sound here."})
    );
    fake.no_sound.store(false, SeqCst);
    send(&mut ws, json!({"t": "audio", "on": true})).await;
    assert_eq!(
        ctl_t(&mut ws, "audio").await,
        json!({"t": "audio", "rate": 48000, "ch": 2})
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn switching_off_mid_session_stops_sound_and_transfers() {
    let (_d, r, mut ws, dl, _stop) = file_session(json!({})).await;
    send(&mut ws, json!({"t": "audio", "on": true})).await;
    assert_eq!(ctl_t(&mut ws, "audio").await["ch"], 2);
    send(
        &mut ws,
        json!({"t": "file", "op": "put", "id": 1, "name": "a.txt", "size": 1000}),
    )
    .await;
    ws.send(up_chunk(1, b"0123456789")).await.unwrap();
    barrier(&mut ws, 1).await;
    until("partial a.txt.part", || dl.join("a.txt.part").exists()).await;
    {
        let mut s = lock(&r.c.settings);
        s["remote"]["audio"] = json!(false);
        s["remote"]["files"] = json!(false);
    }
    // within one stats tick (pinging: the test idle limit is as long as the tick)
    let off = json!({"t": "file", "id": 1, "err": "File transfer was switched off on the device."});
    let (mut sound_off, mut file_off, mut n) = (false, false, 10);
    let until_t = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut pinged = tokio::time::Instant::now();
    while !(sound_off && file_off) {
        assert!(
            tokio::time::Instant::now() < until_t,
            "switched off in time"
        );
        // a ping per message would fill the control queue (it drops when full)
        if pinged.elapsed() >= Duration::from_millis(500) {
            pinged = tokio::time::Instant::now();
            n += 1;
            send(&mut ws, json!({"t": "ping", "n": n})).await;
        }
        let Some(b) = next(&mut ws, 300).await else {
            continue;
        };
        if b[0] == 0x02 {
            let v: Value = serde_json::from_slice(&b[1..]).unwrap();
            sound_off |= v == json!({"t": "audio", "on": false});
            file_off |= v == off;
        }
    }
    until("a.txt.part deleted", || !dl.join("a.txt.part").exists()).await;
    // its chunks are dropped silently, new transfers refused
    ws.send(up_chunk(1, b"x")).await.unwrap();
    send(
        &mut ws,
        json!({"t": "file", "op": "put", "id": 2, "name": "b.txt", "size": 1}),
    )
    .await;
    assert_eq!(
        ctl_t(&mut ws, "file").await,
        json!({"t": "file", "id": 2, "err": "File transfer is switched off on that computer."})
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn file_actions_mark_the_session_controlled() {
    let (_d, r, mut ws, _dl, _stop) = file_session(json!({})).await;
    send(&mut ws, json!({"t": "view", "on": true})).await;
    send(&mut ws, json!({"t": "fs", "op": "list", "path": ""})).await;
    assert!(ctl_t(&mut ws, "fs").await.get("err").is_some());
    assert!(
        !r.live().unwrap().control,
        "a refused listing is not control"
    );
    send(&mut ws, json!({"t": "view", "on": false})).await;
    send(&mut ws, json!({"t": "fs", "op": "list", "path": ""})).await;
    assert!(ctl_t(&mut ws, "fs").await.get("entries").is_some());
    assert!(r.live().unwrap().control);
}

// ---- H.264 ----------------------------------------------------------------------------------

/// The caps a host lists: the fixed ones, `h264` where this machine has an encoder, `more`.
fn with_h264(more: &[&str]) -> Value {
    let mut v = vec!["screens", "cursor", "stats", "takeover", "media", "all"];
    if h264::available() {
        v.push("h264");
    }
    v.extend(more);
    json!(v)
}

/// NAL unit types of one Annex B access unit (after the `0x06` header).
fn nal_types(f: &[u8]) -> Vec<u8> {
    h264::nals(&f[3..]).map(|(t, _)| t).collect()
}

/// Next H.264 frame (`0x06`) within `ms`, skipping control messages.
async fn next_h264(ws: &mut Ws, ms: u64) -> Option<Vec<u8>> {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let b = next(ws, left.as_millis() as u64).await?;
        assert_ne!(b[0], 0x01, "JPEG after H.264 was offered");
        if b[0] == 0x06 {
            return Some(b);
        }
    }
}

/// The platform encoder: IDR with SPS and PPS first, then deltas; live bitrate steps; IDR
/// on request and on a new size. Flags are read over everything that came out, in order (a
/// hardware encoder may hand a frame over one call late).
#[cfg(any(windows, target_os = "macos"))]
#[test]
fn h264_frames_start_with_an_idr_carrying_sps_pps() {
    // a 16x16 block moves over grey: small changes, so deltas
    let img = |n: u32| {
        image::RgbaImage::from_fn(640, 360, |x, y| {
            let v = if (x / 16, y / 16) == (n, 3) { 20 } else { 128 };
            image::Rgba([v, v, v, 255])
        })
    };
    let mut st = h264::State::default();
    let mut out: Vec<Vec<u8>> = Vec::new();
    // (block position, kbps, widest, keyframe asked): steps 1..=3 change the bitrate live,
    // 4 asks for a keyframe, 5 halves the size, 6 pushes out anything still held
    for (n, kbps, max_w, key) in [
        (0, 6_000, u32::MAX, false),
        (1, 6_000, u32::MAX, false),
        (2, 3_000, u32::MAX, false),
        (3, 16_000, u32::MAX, false),
        (4, 16_000, u32::MAX, true),
        (5, 16_000, 320, false),
        (6, 16_000, 320, false),
    ] {
        let f = h264::frame(&mut st, &img(n), 3, kbps, 60, max_w, key).unwrap();
        out.extend(f.into_iter().map(|b| b.to_vec()));
    }
    assert!(out.len() >= 6, "{} frames out", out.len());
    assert!(out.iter().all(|f| f[..2] == [0x06, 3]));
    let flags: Vec<u8> = out.iter().map(|f| f[2]).collect();
    assert_eq!(
        flags[..6],
        [1, 0, 0, 0, 1, 1],
        "IDR, deltas (bitrate live), IDR asked, new size"
    );
    for f in out.iter().filter(|f| f[2] == 1) {
        let t = nal_types(f);
        assert!(t.contains(&7) && t.contains(&8) && t.contains(&5), "{t:?}");
    }
    assert!(out
        .iter()
        .filter(|f| f[2] == 0)
        .all(|f| !nal_types(f).contains(&7)));
    let sps = h264::nals(&out[0][3..])
        .find(|(t, _)| *t == 7)
        .unwrap()
        .1
        .to_vec();
    println!("{}: SPS {}", st.name().unwrap(), h264::sps_info(&sps));
}

/// An encoder that fails is dropped; one failing before its first frame is not tried again,
/// and when none is left `frame` says so (the caller streams JPEG).
#[cfg(any(windows, target_os = "macos"))]
#[test]
fn failing_encoders_are_skipped_then_jpeg() {
    let img = image::RgbaImage::from_pixel(640, 360, image::Rgba([9, 9, 9, 255]));
    let mut st = h264::State::failing();
    let mut calls = 0;
    let err = loop {
        calls += 1;
        assert!(calls < 20, "never gave up");
        match h264::frame(&mut st, &img, 0, 6_000, 60, u32::MAX, false) {
            Ok(f) => assert!(f.is_empty()),
            Err(e) => break e,
        }
    };
    // one call per encoder on this machine, then the error
    assert!(calls >= 2, "{calls}");
    assert!(st.name().is_none());
    println!("gave up after {} encoder(s): {err}", calls - 1);
}

/// The encoder thread falls back to JPEG when no H.264 encoder works and tells the grabber.
#[test]
fn encoder_thread_falls_back_to_jpeg() {
    let slot: Slot = Default::default();
    let (tx, mut rx) = mpsc::channel::<Bytes>(1);
    let (tune, alive) = (Mutex::new(Tune::default()), AtomicBool::new(true));
    let got = std::thread::scope(|sc| {
        sc.spawn(|| {
            let st = h264::State::failing();
            encoder(&slot, &tx, &tune, &alive, st)
        });
        let img = Arc::new(image::RgbaImage::from_pixel(
            640,
            360,
            image::Rgba([9, 9, 9, 255]),
        ));
        let codec = Codec::H264 {
            kbps: 6_000,
            fps: 60,
            key: false,
        };
        let mut got = None;
        // every encoder on the machine is opened and fails first (several on a PC with GPUs)
        for _ in 0..500 {
            offer(&slot, (0, img.clone(), codec, u32::MAX));
            match rx.try_recv() {
                Ok(f) if f[0] == 0x01 => {
                    got = Some(f);
                    break;
                }
                _ => std::thread::sleep(Duration::from_millis(20)),
            }
        }
        alive.store(false, SeqCst);
        got
    });
    assert!(got.is_some(), "no JPEG frame");
    assert!(lock(&tune).jpeg_only);
}

#[test]
fn a_replaced_picture_keeps_its_keyframe_request() {
    let slot: Slot = Default::default();
    let img = Arc::new(image::RgbaImage::new(2, 2));
    let h = |key| Codec::H264 {
        kbps: 1,
        fps: 1,
        key,
    };
    offer(&slot, (0, img.clone(), h(true), 9));
    offer(&slot, (0, img.clone(), h(false), 9));
    assert_eq!(lock(&slot.0).as_ref().unwrap().2, h(true));
    offer(&slot, (0, img.clone(), Codec::Jpeg(70), 9));
    assert_eq!(lock(&slot.0).as_ref().unwrap().2, Codec::Jpeg(70));
    offer(&slot, (0, img, h(false), 9));
    assert_eq!(lock(&slot.0).as_ref().unwrap().2, h(false));
}

#[test]
fn h264_sizes_are_even_and_within_limits() {
    assert_eq!(h264::size(1920, 1080, u32::MAX), (1920, 1080));
    assert_eq!(h264::size(1921, 1081, u32::MAX), (1920, 1080));
    assert_eq!(h264::size(3000, 1000, 1920), (1920, 640));
    assert_eq!(h264::size(5120, 2880, u32::MAX), (3840, 2160));
    assert_eq!(h264::size(2160, 3840, u32::MAX), (2160, 3840));
    assert_eq!(h264::size(7680, 1080, u32::MAX), (3840, 540));
    // tiny pictures are scaled up (hardware encoders refuse them)
    assert_eq!(h264::size(100, 50, u32::MAX), (256, 128));
    let au = [
        0, 0, 0, 1, 0x67, 0x42, 0, 0, 1, 0x68, 0xce, 0, 0, 0, 1, 0x65, 0x88,
    ];
    let n: Vec<_> = h264::nals(&au).collect();
    assert_eq!(
        n,
        [
            (7, &[0x67, 0x42][..]),
            (8, &[0x68, 0xce][..]),
            (5, &[0x65, 0x88][..])
        ]
    );
}

#[test]
fn h264_bitrate_and_scale_follow_the_mode() {
    assert_eq!(Quality::Quality.h264(1, 3840), (25_000, u32::MAX));
    assert_eq!(Quality::Speed.h264(1, 3840), (6_000, 1280));
    assert_eq!(Quality::Auto.h264(1, 3840), (16_000, 1920));
    // auto steps the bitrate down first, the scale only at the last step
    assert_eq!(Quality::Auto.h264(3, 1600), (6_000, 1600));
    assert_eq!(Quality::Auto.h264(99, 1600), (3_000, 1200));
    let mut t = Tune {
        coder: Some("NVENC via MF".into()),
        ..Tune::default()
    };
    let s = t.tick(Quality::Auto, 3840);
    assert_eq!(
        (&s["codec"], &s["br"], &s["scale"], &s["encoder"]),
        (
            &json!("h264"),
            &json!(16_000),
            &json!(0.5),
            &json!("NVENC via MF")
        )
    );
    assert!(s.get("q").is_none());
    t.on_encoded(Duration::from_millis(1), 1000, None);
    let s = t.tick(Quality::Auto, 1000);
    assert_eq!((&s["codec"], &s["q"]), (&json!("jpeg"), &json!(70)));
}

/// The encoder thread hands over every frame it encoded, in order; a picture is only ever
/// replaced before it is encoded (shown with JPEG, whose frames say what they show).
#[test]
fn encoded_frames_are_never_dropped() {
    let slot: Slot = Default::default();
    let (tx, mut rx) = mpsc::channel::<Bytes>(1);
    let (tune, alive) = (Mutex::new(Tune::default()), AtomicBool::new(true));
    let put = |n: u8| {
        let img = image::RgbaImage::from_pixel(64, 32, image::Rgba([n, n, n, 255]));
        *lock(&slot.0) = Some((0, Arc::new(img), Codec::Jpeg(90), u32::MAX));
        slot.1.notify_one();
    };
    let taken = || {
        for _ in 0..500 {
            if lock(&slot.0).is_none() {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("the encoder never took the picture");
    };
    let got = std::thread::scope(|sc| {
        sc.spawn(|| encoder(&slot, &tx, &tune, &alive, Default::default()));
        put(40);
        taken();
        put(80); // encoded, then waits: the channel holds the first
        taken();
        std::thread::sleep(Duration::from_millis(50));
        put(120); // never encoded: replaced by the next one
        put(200);
        let got: Vec<_> = (0..3).map(|_| rx.blocking_recv().unwrap()).collect();
        alive.store(false, SeqCst);
        got
    });
    let shades: Vec<u8> = got
        .iter()
        .map(|f| {
            image::load_from_memory(&f[2..])
                .unwrap()
                .to_rgba8()
                .get_pixel(0, 0)[0]
        })
        .collect();
    for (s, n) in shades.iter().zip([40u8, 80, 200]) {
        assert!(s.abs_diff(n) <= 3, "{shades:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn h264_negotiated_and_keyframes_on_request() {
    if !h264::available() {
        return; // nothing to negotiate on this machine
    }
    let (_d, r, fake) = setup(true);
    let (addr, _stop) = serve_local(&r).await;
    let token = paired_token(&r, &addr).await;
    let mut ws = open(&addr, &auth(&token)).await;
    let hello = next_ctl(&mut ws).await;
    assert!(hello["caps"].as_array().unwrap().contains(&json!("h264")));
    // not offered (older viewers): JPEG
    send(&mut ws, json!({"t": "start", "screen": 0, "maxFps": 60})).await;
    assert_eq!(&next_frame(&mut ws, 3000).await.unwrap()[..2], &[0x01, 0]);
    send(&mut ws, json!({"t": "ack"})).await;
    // offered: H.264 from the next picture on, starting with an IDR
    send(
        &mut ws,
        json!({"t": "start", "screen": 0, "maxFps": 60, "codecs": ["h264"]}),
    )
    .await;
    let first = loop {
        let b = next(&mut ws, 5000).await.expect("a frame");
        match b[0] {
            0x01 => send(&mut ws, json!({"t": "ack"})).await, // JPEG still in flight
            0x06 => break b,
            _ => {}
        }
    };
    assert_eq!(&first[..3], &[0x06, 0, 1]);
    send(&mut ws, json!({"t": "ack"})).await;
    let f = next_h264(&mut ws, 3000).await.unwrap();
    assert_eq!(f[2], 0, "then deltas");
    send(&mut ws, json!({"t": "ack"})).await;
    // a still screen: the last picture a few more times (sharpening it), then nothing...
    fake.same.store(true, SeqCst);
    fake.still.store(true, SeqCst);
    // every wait stays under the session's 2 s idle limit (this fake viewer only acks, it
    // never pings): a longer silence ends the session
    let mut n = 0;
    loop {
        // the repeats after the last change are owed: a slow machine only delays them
        let wait = if n < REFINE as usize { 1500 } else { 300 };
        let Some(_) = next_h264(&mut ws, wait).await else {
            break;
        };
        n += 1;
        send(&mut ws, json!({"t": "ack"})).await;
        // repeats that never stop would never leave a quiet gap
        assert!(n <= 3 * REFINE as usize, "the repeats never stop");
    }
    // at least the repeats after the last change (more may have been in flight)
    assert!(n >= REFINE as usize, "{n}");
    // ...until the viewer asks for a keyframe; a repeat that came after the quiet gap (a
    // slow machine) may still come first
    send(&mut ws, json!({"t": "idr"})).await;
    let f = loop {
        let f = next_h264(&mut ws, 1500)
            .await
            .expect("a keyframe on request");
        if f[2] == 1 {
            break f;
        }
        send(&mut ws, json!({"t": "ack"})).await;
        n += 1;
        assert!(n <= 3 * REFINE as usize, "no keyframe on request");
    };
    assert!(nal_types(&f).contains(&7) && nal_types(&f).contains(&8));
    send(&mut ws, json!({"t": "ack"})).await;
    let stats = loop {
        let v = next_ctl(&mut ws).await;
        if v["t"] == "stats" {
            break v;
        }
    };
    // frames left unacknowledged for a while: auto may have stepped down already
    assert_eq!(stats["codec"], "h264", "{stats}");
    assert!(stats["encoder"].is_string(), "{stats}");
    assert!(
        matches!(stats["br"].as_u64(), Some(10_000 | 16_000)),
        "{stats}"
    );
}

/// The platform encoder on a 1920x1080 picture (scale, conversion and encode, no capture):
/// every encoder found, the one picked, ms per frame when the whole screen scrolls every
/// frame (worst case) and when a small area changes (typing).
/// `cargo test --release --lib h264_speed -- --ignored --nocapture`
#[cfg(any(windows, target_os = "macos"))]
#[test]
#[ignore]
fn h264_speed() {
    println!("encoders: {:#?}", h264::encoders());
    // screen-like: flat areas, sharp edges, text-sized detail
    let base = image::RgbaImage::from_fn(1920, 1080 + 8 * 120, |x, y| {
        let v = if (x / 7 + y / 11) % 5 == 0 { 30 } else { 230 };
        image::Rgba([v, (x % 256) as u8, (y % 200) as u8 + 30, 255])
    });
    let still = image::imageops::crop_imm(&base, 0, 0, 1920, 1080).to_image();
    for (name, scroll) in [("scroll 8 px/frame", true), ("typing (small area)", false)] {
        let mut st = h264::State::default();
        let mut ms = Vec::new();
        let mut frames = 0;
        for i in 0..121u32 {
            let img = if scroll {
                image::imageops::crop_imm(&base, 0, i * 8, 1920, 1080).to_image()
            } else {
                let mut img = still.clone();
                let block = image::RgbaImage::from_pixel(12, 18, image::Rgba([0, 0, 0, 255]));
                image::imageops::overlay(&mut img, &block, i64::from(100 + (i % 60) * 12), 500);
                img
            };
            let t = Instant::now();
            frames += h264::frame(&mut st, &img, 0, 16_000, 60, u32::MAX, false)
                .unwrap()
                .len();
            if i > 0 {
                ms.push(t.elapsed().as_secs_f64() * 1000.0); // the first opens the encoder
            }
        }
        ms.sort_by(f64::total_cmp);
        let avg = ms.iter().sum::<f64>() / ms.len() as f64;
        println!(
            "{} {name}, 16 Mbps: {avg:.1} ms/frame avg, p50 {:.1}, p95 {:.1}, {frames}/121 frames out",
            st.name().unwrap_or("?"),
            ms[ms.len() / 2],
            ms[ms.len() * 95 / 100]
        );
    }
}

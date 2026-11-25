use super::*;
use crypto_secretbox::aead::{Aead, KeyInit};
use crypto_secretbox::{Nonce, XSalsa20Poly1305};
use sha2::{Digest, Sha512};

// A real cookie's shape (`normalize_cookie` wants 100+ header-safe characters).
const COOKIE: &str = "_|WARNING:-test-cookie-not-real-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef-padding";

fn payload() -> Vec<u8> {
    serde_json::json!([
        {
            "SecurityToken": COOKIE, "Username": "Alice", "UserID": 42,
            "Alias": "al", "Description": "main", "Group": "Default", "Password": "pw1",
            "LastUse": "2024-05-06T07:08:09Z", "BrowserTrackerID": "123456789",
            "Fields": {
                "SavedPlaceId": "606849621", "SavedJobId": "job-1", "MaxFPS": "60",
                "Window_X": "10", "Window_Y": "-20", "Window_Width": "800", "Window_Height": "600",
                "Custom": "v"
            }
        },
        { "SecurityToken": "_|WARNING:-test-2", "Username": "Bob", "UserID": "7", "Group": "Alts",
          "LastUse": "5/6/2024 7:08:09 PM", "Fields": { "Window_X": "1", "SavedJobId": "job-2" }, "Unknown": 1 },
        { "no": "username" }
    ])
    .to_string()
    .into_bytes()
}

/// Test-side encryptor of the same scheme (independent of `unlock`'s decrypt path).
fn lock(plain: &[u8], password: &str) -> Vec<u8> {
    let (salt, nonce) = ([7u8; SALT_LEN], [9u8; NONCE_LEN]);
    let pre = Sha512::digest(password.as_bytes());
    let mut key = [0u8; 32];
    let params = argon2::Params::new(262_144, 3, 1, Some(32)).unwrap();
    argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
        .hash_password_into(&pre, &salt, &mut key)
        .unwrap();
    let ct = XSalsa20Poly1305::new((&key).into())
        .encrypt(Nonce::from_slice(&nonce), plain)
        .unwrap();
    [MAGIC.as_slice(), &salt, &nonce, &ct].concat()
}

#[test]
fn header_is_64_bytes_ending_in_7_dots() {
    assert_eq!(MAGIC.len(), 64);
    assert!(MAGIC.ends_with(b"......."));
    assert!(!MAGIC.ends_with(b"........"));
}

#[test]
fn locked_roundtrip_and_wrong_password() {
    let file = lock(&payload(), "hunter2");
    let got = decrypt(&file, Some("hunter2")).unwrap().0;
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].cookie.as_deref().map(String::as_str), Some(COOKIE));
    assert_eq!(
        decrypt(&file, Some("nope")).unwrap_err(),
        ImportError::WrongPassword
    );
    assert_eq!(
        decrypt(&file, None).unwrap_err(),
        ImportError::PasswordRequired
    );
}

#[test]
fn truncated_and_garbage() {
    let mut f = MAGIC.to_vec();
    f.extend([0u8; 20]);
    assert_eq!(decrypt(&f, Some("x")).unwrap_err(), ImportError::Truncated);
    assert_eq!(
        decrypt(b"hello", None).unwrap_err(),
        ImportError::Unrecognised
    );
    assert_eq!(
        decrypt(b"{\"a\":1}", None).unwrap_err(),
        ImportError::Unrecognised
    );
}

#[test]
fn plain_mapping() {
    let (got, skipped) = decrypt(&payload(), None).unwrap();
    assert_eq!(got.len(), 2, "entry without Username skipped");
    assert_eq!(skipped, 1);
    let a = &got[0];
    assert_eq!(a.user_id, Some(42));
    assert_eq!(a.group, "Unsorted");
    assert_eq!(a.alias.as_deref(), Some("al"));
    assert_eq!(a.note.as_deref(), Some("main"));
    assert_eq!(a.password.as_deref().map(String::as_str), Some("pw1"));
    assert_eq!(a.tracker_id.as_deref(), Some("123456789"));
    assert_eq!(
        a.launch_target,
        Some(LaunchTarget {
            place_id: Some(606849621),
            job_id: Some("job-1".into())
        })
    );
    assert_eq!(a.fps_cap, Some(60));
    assert_eq!(
        a.window_rect,
        Some(WindowRect {
            x: 10,
            y: -20,
            w: 800,
            h: 600
        })
    );
    assert_eq!(a.last_launched_at.as_deref(), Some("2024-05-06T07:08:09Z"));
    assert_eq!(a.extra.len(), 1);
    assert_eq!(a.extra["Custom"], "v");

    let b = &got[1];
    assert_eq!(b.user_id, Some(7));
    assert_eq!(b.group, "Alts");
    assert_eq!(b.window_rect, None, "partial window set ignored");
    assert_eq!(b.launch_target, None, "job id without a place is no target");
    assert_eq!(b.last_launched_at.as_deref(), Some("2024-05-06T19:08:09Z"));
}

#[test]
fn names_cleaned_cookies_checked_caps_held() {
    let raw = serde_json::json!([
        { "Username": "\u{202e}evil\u{200b}", "SecurityToken": format!(".ROBLOSECURITY={COOKIE};"),
          "Fields": { "MaxFPS": "500" } },
        { "Username": "Ok", "SecurityToken": "short" },
        { "Username": "\u{202e}\u{200b}" }
    ]);
    let (got, skipped) = decrypt(raw.to_string().as_bytes(), None).unwrap();
    assert_eq!(
        (got.len(), skipped),
        (2, 1),
        "an all-invisible name is skipped"
    );
    assert_eq!(got[0].username, "evil");
    assert_eq!(got[0].cookie.as_deref().map(String::as_str), Some(COOKIE));
    assert_eq!(got[0].fps_cap, Some(240));
    assert!(got[1].cookie.is_none(), "malformed cookie dropped");
    let many = format!(
        "[{}]",
        vec![r#"{"Username":"u"}"#; MAX_ENTRIES + 1].join(",")
    );
    assert_eq!(
        decrypt(many.as_bytes(), None).unwrap_err(),
        ImportError::TooMany
    );
}

#[test]
fn debug_masks_secrets() {
    let got = decrypt(&payload(), None).unwrap().0;
    let d = format!("{:?}", got[0]);
    assert!(!d.contains(COOKIE) && !d.contains("pw1"));
}

#[test]
fn detect_files_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    assert!(detect(dir.path()).account_data.is_none());
    let p = dir.path().join("AccountData.json");
    std::fs::write(&p, lock_header_only()).unwrap();
    std::fs::write(dir.path().join("RAMSettings.ini"), "").unwrap();
    let before = std::fs::metadata(&p).unwrap().modified().unwrap();
    let d = detect(dir.path());
    assert_eq!(d.account_data, Some((p.clone(), Ok(Encoding::Locked))));
    assert!(d.settings.is_some() && d.recents.is_none() && d.favorites.is_none());
    assert_eq!(std::fs::metadata(&p).unwrap().modified().unwrap(), before);

    std::fs::write(&p, payload()).unwrap();
    assert_eq!(
        detect(dir.path()).account_data.unwrap().1,
        Ok(Encoding::Plain)
    );
}

fn lock_header_only() -> Vec<u8> {
    MAGIC.to_vec()
}

#[cfg(windows)]
#[test]
fn dpapi_roundtrip() {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{CryptProtectData, CRYPT_INTEGER_BLOB};
    let data = payload();
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let ent = CRYPT_INTEGER_BLOB {
        cbData: ENTROPY.len() as u32,
        pbData: ENTROPY.as_ptr() as *mut u8,
    };
    let mut out = CRYPT_INTEGER_BLOB::default();
    let blob = unsafe {
        CryptProtectData(&input, None, Some(&ent), None, None, 0, &mut out).unwrap();
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(out.pbData as *mut _)));
        v
    };
    assert_eq!(decrypt(&blob, None).unwrap().0.len(), 2);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("AccountData.json"), &blob).unwrap();
    assert_eq!(
        detect(dir.path()).account_data.unwrap().1,
        Ok(Encoding::Dpapi)
    );
    // Corrupt blob -> user-readable DPAPI error.
    let mut bad = blob.clone();
    *bad.last_mut().unwrap() ^= 1;
    assert_eq!(decrypt(&bad, None).unwrap_err(), ImportError::Dpapi);
}

#[test]
fn ini_heuristics() {
    let p = parse_ini(
        "[General]\n; c\nLaunchDelay=5\nUnlockFPS=true\nFPSValue=144\nAllowMultiClient=1\nMaxRecentGames=20\nOther=x\n",
    );
    assert_eq!(
        p,
        LegacyPrefs {
            launch_delay_ms: Some(5000),
            fps_unlock: Some(true),
            fps_value: Some(144),
            multi_client: Some(true),
            recents_max: Some(20)
        }
    );
    assert_eq!(parse_ini("DelayMs=750").launch_delay_ms, Some(750));
    assert_eq!(parse_ini("garbage\n=\nx=y"), LegacyPrefs::default());
}

#[test]
fn games_variants() {
    let g = parse_games_json(
        r#"[{"PlaceId":1,"Name":"A","ImageUrl":"u"},{"placeId":"2","name":"B","Thumbnail":"t"},{"Id":3},{"Name":"no id"}]"#,
    );
    assert_eq!(g.len(), 3);
    assert_eq!(
        (g[0].place_id, g[0].name.as_str(), g[0].image_url.as_deref()),
        (1, "A", Some("u"))
    );
    assert_eq!((g[1].place_id, g[1].image_url.as_deref()), (2, Some("t")));
    assert_eq!(g[2].image_url, None);
    assert!(parse_games_json("not json").is_empty());
}

#[test]
fn libsodium_kat() {
    use base64::Engine;
    let kat: Value = serde_json::from_str(include_str!("kat_locked.json")).unwrap();
    let file = base64::engine::general_purpose::STANDARD
        .decode(kat["file_b64"].as_str().unwrap())
        .unwrap();
    let got = decrypt(&file, kat["password"].as_str()).unwrap().0;
    assert_eq!(got.len(), 1);
    let a = &got[0];
    assert_eq!((a.username.as_str(), a.user_id), ("KatUser", Some(12345)));
    assert_eq!(a.tracker_id.as_deref(), Some("987654321012"));
    assert_eq!(a.group, "Unsorted");
    assert_eq!(a.launch_target.as_ref().unwrap().place_id, Some(606849621));
}

#[test]
fn size_cap_and_unrecognised_detect() {
    let big = vec![b' '; MAX_FILE as usize + 1];
    assert_eq!(decrypt(&big, None).unwrap_err(), ImportError::TooLarge);
    assert_eq!(
        parse_ini(&" ".repeat(MAX_FILE as usize + 1)),
        LegacyPrefs::default()
    );
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("AccountData.json");
    std::fs::write(&p, &big).unwrap();
    assert_eq!(
        detect(dir.path()).account_data.unwrap().1,
        Err(ImportError::TooLarge)
    );
    std::fs::write(&p, b"junk").unwrap();
    assert_eq!(
        detect(dir.path()).account_data.unwrap().1,
        Err(ImportError::Unrecognised)
    );
}

#[test]
fn mapping_limits_and_window_leftovers() {
    let long = "a".repeat(6000);
    let j = serde_json::json!([{
        "Username": "u", "Alias": long, "Description": long,
        "Fields": { "MaxFPS": "5000", "Window_X": "1", "Window_Y": "2", "Window_W": "3", "Window_Height": "x" }
    }, {
        "Username": "v", "Fields": { "MaxFPS": "0", "Window_X": "1", "Window_Y": "2", "Window_W": "3", "Window_H": "4" }
    }])
    .to_string();
    let got = decrypt(j.as_bytes(), None).unwrap().0;
    assert_eq!(got[0].alias.as_ref().unwrap().len(), 50);
    assert_eq!(got[0].note.as_ref().unwrap().len(), 5000);
    assert_eq!(got[0].fps_cap, None);
    assert_eq!(got[0].window_rect, None);
    assert_eq!(got[0].extra.len(), 4);
    assert_eq!(got[1].fps_cap, None);
    assert_eq!(
        got[1].window_rect,
        Some(WindowRect {
            x: 1,
            y: 2,
            w: 3,
            h: 4
        })
    );
    assert!(got[1].extra.is_empty());
}

#[test]
fn plain_bom_and_leading_space() {
    let mut f: Vec<u8> = vec![0xEF, 0xBB, 0xBF, b' ', b' '];
    f.extend(payload());
    assert_eq!(sniff(&f), Ok(Encoding::Plain));
    assert_eq!(decrypt(&f, None).unwrap().0.len(), 2);
}

#[test]
fn missing_file_is_unreadable() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(
        read_capped(&d.path().join("gone.json")),
        Err(ImportError::Unreadable)
    );
}

#[test]
fn games_bom_and_non_array() {
    let g = parse_games_json("\u{feff}[{\"PlaceId\":5}]");
    assert_eq!((g.len(), g[0].place_id, g[0].name.as_str()), (1, 5, ""));
    assert!(parse_games_json(r#"{"PlaceId":1}"#).is_empty());
    assert!(parse_games_json(r#"[{"PlaceId":"x"}]"#).is_empty());
}

#[test]
fn ini_edge_values() {
    // negative delay skipped, the next usable key wins; seconds may be fractional
    assert_eq!(
        parse_ini("LaunchDelay=-1\nDelayMs=500").launch_delay_ms,
        Some(500)
    );
    assert_eq!(parse_ini("LaunchDelay=2.5").launch_delay_ms, Some(2500));
    // first usable value wins per key; junk is skipped, not stored
    let p = parse_ini("MultiClient=off\nAllowMulti=yes\nFPSValue=abc\nFPSValue=60\nFPSValue=144");
    assert_eq!((p.multi_client, p.fps_value), (Some(false), Some(60)));
    assert_eq!(parse_ini("FPSUnlock=maybe").fps_unlock, None);
}

#[test]
fn window_values_out_of_range_stay_as_extra() {
    // a negative size cannot be a u32: no rect, every Window_* key kept as extra
    let j = r#"[{"Username":"u","Fields":{"Window_X":"1","Window_Y":"2","Window_W":"-3","Window_H":"4"}}]"#;
    let got = decrypt(j.as_bytes(), None).unwrap().0;
    assert_eq!(got[0].window_rect, None);
    assert_eq!(got[0].extra.len(), 4);
}

#[test]
fn bad_user_id_empty_name_and_dates() {
    let j = r#"[
        {"Username":"","UserID":"5"},
        {"Username":"a","UserID":"abc","LastUse":"not a date","Fields":{"MaxFPS":" 90 ","Flag":true}},
        {"Username":"b","LastUse":"2024-05-06 07:08:09"}
    ]"#;
    let (got, skipped) = decrypt(j.as_bytes(), None).unwrap();
    assert_eq!((got.len(), skipped), (2, 1));
    assert_eq!(got[0].user_id, None);
    assert_eq!(got[0].last_launched_at, None);
    assert_eq!(got[0].fps_cap, Some(90));
    assert!(got[0].extra.is_empty(), "bool field ignored");
    assert_eq!(
        got[1].last_launched_at.as_deref(),
        Some("2024-05-06T07:08:09Z")
    );
}

#[test]
fn dpapi_version_bytes_sniffed() {
    assert_eq!(sniff(&[1, 0, 0, 0, 0xAB]), Ok(Encoding::Dpapi));
    assert_eq!(sniff(&[1, 0, 0]), Err(ImportError::Unrecognised));
}

#[cfg(not(windows))]
#[test]
fn dpapi_file_is_unsupported_off_windows() {
    assert_eq!(
        decrypt(&[1, 0, 0, 0, 0xAB], None).unwrap_err(),
        ImportError::DpapiUnsupported
    );
}

#[test]
fn dates_offsets_12h_and_fractions() {
    assert_eq!(
        parse_date("2024-05-06T09:08:09+02:00").as_deref(),
        Some("2024-05-06T07:08:09Z")
    );
    assert_eq!(
        parse_date("5/6/2024 19:08:09").as_deref(),
        Some("2024-05-06T19:08:09Z")
    );
    assert_eq!(
        parse_date("5/6/2024 7:08:09 PM").as_deref(),
        Some("2024-05-06T19:08:09Z")
    );
    assert_eq!(
        parse_date("2024-05-06T07:08:09.999").as_deref(),
        Some("2024-05-06T07:08:09Z")
    );
}

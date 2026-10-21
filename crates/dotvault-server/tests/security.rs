mod common;

use std::time::Duration;

use axum::http::StatusCode;
use common::*;
use dotvault_core::api::{CreateItemRequest, LoginRequest, RegisterRequest};
use dotvault_server::{config::RateLimit, harness, Config};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const PW: &str = "correct horse battery staple";

// --- authentication is required everywhere ---------------------------------------

#[tokio::test]
async fn protected_routes_reject_missing_and_bad_credentials() {
    let app = TestApp::new().await;
    let alice = app.register("alice", PW).await;
    let id = new_item_id();
    let item = format!("/v1/vault/items/{id}");
    let share = "/v1/shares/AAAAAAAAAAAAAAAAAAAAAA";

    let routes: Vec<(&str, String)> = vec![
        ("GET", "/v1/me".into()),
        ("POST", "/v1/auth/logout".into()),
        ("POST", "/v1/auth/password".into()),
        ("GET", "/v1/vault/items".into()),
        ("POST", "/v1/vault/items".into()),
        ("GET", item.clone()),
        ("PUT", item.clone()),
        ("DELETE", item.clone()),
        ("GET", "/v1/shares".into()),
        ("POST", "/v1/shares".into()),
        ("POST", format!("{share}/open")),
        ("DELETE", share.into()),
        ("GET", format!("{share}/events")),
        ("POST", format!("{share}/events")),
    ];

    let expired = {
        sqlx::query("UPDATE sessions SET expires_at = 1")
            .execute(&app.state.db)
            .await
            .unwrap();
        alice.token.clone()
    };

    for (method, path) in &routes {
        let attempts: Vec<(&str, Req)> = vec![
            ("no header", Req::new(method, path)),
            ("garbage token", Req::new(method, path).token("garbage")),
            (
                "well-formed unknown token",
                Req::new(method, path).token(&b64(&[3u8; 32])),
            ),
            (
                "wrong scheme",
                Req::new(method, path).header("authorization", &format!("Basic {expired}")),
            ),
            (
                "empty bearer",
                Req::new(method, path).header("authorization", "Bearer "),
            ),
            ("expired session", Req::new(method, path).token(&expired)),
        ];
        for (why, req) in attempts {
            let resp = app.send(req).await;
            assert_eq!(
                (resp.status, resp.error().code.as_str()),
                (StatusCode::UNAUTHORIZED, "unauthorized"),
                "{method} {path} ({why})"
            );
        }
    }
}

// --- rate limiting ----------------------------------------------------------------

#[tokio::test]
async fn auth_endpoints_share_a_per_ip_budget() {
    let app = TestApp::with_config(|c| c.auth_rate = RateLimit::per_minute(3)).await;
    let prelogin = |ip: &str| Req::new("GET", "/v1/auth/prelogin?username=ghost").peer(ip);

    for _ in 0..3 {
        assert_eq!(
            app.send(prelogin("198.51.100.1")).await.status,
            StatusCode::OK
        );
    }
    let blocked = app.send(prelogin("198.51.100.1")).await;
    blocked.assert_error(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    let retry: u64 = blocked
        .header("retry-after")
        .expect("Retry-After")
        .parse()
        .unwrap();
    assert!((1..=60).contains(&retry), "retry-after {retry}");

    // a different client is unaffected
    assert_eq!(
        app.send(prelogin("198.51.100.2")).await.status,
        StatusCode::OK
    );

    // the budget is shared by every limited auth endpoint, checked before the body is even read
    let junk = || Vec::from(&b"{}"[..]);
    let limited = [
        ("POST", "/v1/auth/login"),
        ("POST", "/v1/auth/register"),
        ("POST", "/v1/auth/recover"),
        ("POST", "/v1/auth/recover/unlock"),
    ];
    for (m, p) in limited {
        let resp = app
            .send(
                Req::new(m, p)
                    .peer("198.51.100.1")
                    .raw(junk(), "application/json"),
            )
            .await;
        assert_eq!(resp.status, StatusCode::TOO_MANY_REQUESTS, "{p}");
    }

    // everything else stays reachable from the blocked address
    let healthz = app
        .send(Req::new("GET", "/healthz").peer("198.51.100.1"))
        .await;
    assert_eq!(healthz.status, StatusCode::OK);
    let me = app
        .send(Req::new("GET", "/v1/me").peer("198.51.100.1"))
        .await;
    assert_eq!(me.status, StatusCode::UNAUTHORIZED, "401, not 429");

    // 429s are the same JSON shape as every other error
    assert_eq!(blocked.error().code, "rate_limited");
}

#[tokio::test]
async fn login_attempts_are_counted_even_when_they_fail() {
    let app = TestApp::with_config(|c| c.auth_rate = RateLimit::per_minute(5)).await;
    let attempt = || {
        Req::new("POST", "/v1/auth/login").json(&LoginRequest {
            username: "alice".into(),
            auth_key: vec![1; 32],
            device_name: None,
        })
    };
    for _ in 0..5 {
        assert_eq!(app.send(attempt()).await.status, StatusCode::UNAUTHORIZED);
    }
    app.send(attempt())
        .await
        .assert_error(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
}

#[tokio::test]
async fn share_open_has_its_own_budget() {
    let app = TestApp::with_config(|c| c.share_open_rate = RateLimit::per_minute(2)).await;
    let alice = app.register("alice", PW).await;
    let bob = app.register("bob", PW).await;
    let (req, _) = share_request("x", None, None);
    let created: dotvault_core::api::CreateShareResponse = app
        .post("/v1/shares", Some(&alice.token), &req)
        .await
        .json();
    let open = |ip: &str, token: &str| {
        Req::new("POST", &format!("/v1/shares/{}/open", created.id))
            .token(token)
            .peer(ip)
    };

    assert_eq!(
        app.send(open("192.0.2.10", &bob.token)).await.status,
        StatusCode::OK
    );
    assert_eq!(
        app.send(open("192.0.2.10", &bob.token)).await.status,
        StatusCode::OK
    );
    let blocked = app.send(open("192.0.2.10", &bob.token)).await;
    blocked.assert_error(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    assert!(blocked.header("retry-after").is_some());

    // a limited request never reaches the handler, so it is not a view
    let views: i64 = sqlx::query_scalar("SELECT view_count FROM shares")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(views, 2);

    // other clients and other routes are unaffected
    assert_eq!(
        app.send(open("192.0.2.11", &bob.token)).await.status,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            Req::new("GET", "/v1/shares")
                .token(&alice.token)
                .peer("192.0.2.10")
        )
        .await
        .status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn forwarded_for_is_ignored_unless_the_proxy_is_trusted() {
    // untrusted: a client cannot dodge the limit by lying in X-Forwarded-For
    let app = TestApp::with_config(|c| c.auth_rate = RateLimit::per_minute(3)).await;
    for i in 0..3 {
        let r = Req::new("GET", "/v1/auth/prelogin?username=a")
            .peer("10.1.1.1")
            .header("x-forwarded-for", &format!("203.0.113.{i}"));
        assert_eq!(app.send(r).await.status, StatusCode::OK);
    }
    let r = Req::new("GET", "/v1/auth/prelogin?username=a")
        .peer("10.1.1.1")
        .header("x-forwarded-for", "203.0.113.99");
    assert_eq!(app.send(r).await.status, StatusCode::TOO_MANY_REQUESTS);

    // trusted: the proxy's appended address identifies the client
    let app = TestApp::with_config(|c| {
        c.auth_rate = RateLimit::per_minute(3);
        c.trust_proxy = true;
    })
    .await;
    let via_proxy = |forwarded: &str| {
        Req::new("GET", "/v1/auth/prelogin?username=a")
            .peer("10.1.1.1") // always the proxy
            .header("x-forwarded-for", forwarded)
    };
    // client-supplied junk before the proxy's entry does not matter
    for i in 0..3 {
        let r = via_proxy(&format!("{i}.{i}.{i}.{i}, 203.0.113.7"));
        assert_eq!(app.send(r).await.status, StatusCode::OK);
    }
    assert_eq!(
        app.send(via_proxy("9.9.9.9, 203.0.113.7")).await.status,
        StatusCode::TOO_MANY_REQUESTS
    );
    // a different real client behind the same proxy has its own budget
    assert_eq!(
        app.send(via_proxy("203.0.113.8")).await.status,
        StatusCode::OK
    );
}

// --- body size --------------------------------------------------------------------

#[tokio::test]
async fn oversized_bodies_are_rejected_with_413() {
    let app = TestApp::new().await;
    let alice = app.register("alice", PW).await;
    let huge = |extra: usize| {
        // valid-looking JSON that is simply too long
        let mut body = br#"{"payload_ct":""#.to_vec();
        body.extend(std::iter::repeat_n(b'A', 512 * 1024 + extra));
        body.extend_from_slice(br#""}"#);
        body
    };

    // unauthenticated endpoint
    let r = app
        .send(Req::new("POST", "/v1/auth/register").raw(huge(1), "application/json"))
        .await;
    r.assert_error(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large");

    // authenticated endpoints
    let r = app
        .send(
            Req::new("POST", "/v1/vault/items")
                .token(&alice.token)
                .raw(huge(1), "application/json"),
        )
        .await;
    r.assert_error(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large");
    let r = app
        .send(
            Req::new("POST", "/v1/shares")
                .token(&alice.token)
                .raw(huge(1), "application/json"),
        )
        .await;
    r.assert_error(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large");

    // a well-formed request whose ciphertext field is over its own limit is 413 too
    let req = CreateItemRequest {
        id: new_item_id(),
        name_ct: vec![0; 16],
        payload_ct: vec![0; 300 * 1024],
    };
    app.post("/v1/vault/items", Some(&alice.token), &req)
        .await
        .assert_error(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large");

    // nothing was stored
    assert_eq!(app.count("vault_items").await, 0);
    assert_eq!(app.count("shares").await, 0);
}

// --- the server only ever holds ciphertext -------------------------------------------

fn read_db_files(app: &TestApp) -> Vec<u8> {
    let mut all = Vec::new();
    for suffix in ["", "-wal", "-shm"] {
        let mut name = app.db_path.clone().into_os_string();
        name.push(suffix);
        if let Ok(bytes) = std::fs::read(std::path::PathBuf::from(name)) {
            all.extend(bytes);
        }
    }
    all
}

/// Raw, base64url and hex spellings of a secret.
fn spellings(secret: &[u8]) -> Vec<(String, Vec<u8>)> {
    vec![
        ("raw".into(), secret.to_vec()),
        ("base64url".into(), b64(secret).into_bytes()),
        ("hex".into(), hex(secret).into_bytes()),
    ]
}

#[tokio::test]
async fn database_contains_ciphertext_only() {
    const MARKER: &str = "TOPSECRET-MARKER-5c1e9d0a7b34";
    const PASSWORD: &str = "pw-never-stored-91d7c2e4";

    let app = TestApp::new().await;
    let alice = app.register("alice", PASSWORD).await;
    let bob = app.register("bob", PW).await;

    // a vault item whose name and payload both contain the marker
    let id = new_item_id();
    let payload = format!("API_KEY={MARKER}\nOTHER=1\n");
    let req = create_item_request(
        &alice.vault_key,
        &id,
        &format!("project/{MARKER}"),
        &payload,
    );
    assert_eq!(
        app.post("/v1/vault/items", Some(&alice.token), &req)
            .await
            .status,
        StatusCode::CREATED
    );
    // ...updated once, so a second version exists as well
    let (name_ct, payload_ct) = sealed_item(
        &alice.vault_key,
        &id,
        &format!("project/{MARKER}"),
        &format!("API_KEY={MARKER}-v2\n"),
        2,
    );
    let upd = dotvault_core::api::UpdateItemRequest {
        name_ct,
        payload_ct,
        version: 2,
    };
    assert_eq!(
        app.put(&format!("/v1/vault/items/{id}"), Some(&alice.token), &upd)
            .await
            .status,
        StatusCode::OK
    );

    // a share containing it, opened and saved by bob
    let (share_req, share_key) = share_request(
        &format!("{{\"entries\":{{\"K\":\"{MARKER}\"}}}}"),
        None,
        None,
    );
    let created: dotvault_core::api::CreateShareResponse = app
        .post("/v1/shares", Some(&alice.token), &share_req)
        .await
        .json();
    let opened = app
        .post(
            &format!("/v1/shares/{}/open", created.id),
            Some(&bob.token),
            &serde_json::json!({}),
        )
        .await;
    assert_eq!(opened.status, StatusCode::OK);

    // a login, a password change and a wrong attempt, for good measure
    let session = app.login("alice", PASSWORD).await;
    app.try_login("alice", "wrong-password-guess-77").await;

    // everything the client holds that must never reach the server
    let mut secrets: Vec<(&str, Vec<u8>)> = vec![
        ("marker", MARKER.as_bytes().to_vec()),
        ("password", PASSWORD.as_bytes().to_vec()),
        ("vault key", alice.vault_key.to_vec()),
        ("bob's vault key", bob.vault_key.to_vec()),
        ("recovery key", alice.recovery_key.to_vec()),
        (
            "recovery key text",
            alice.recovery_key_text.clone().into_bytes(),
        ),
        ("auth key", alice.auth_key.to_vec()),
        ("recovery auth key", alice.recovery_auth_key.to_vec()),
        ("share key", share_key.to_vec()),
        ("session token (string)", alice.token.clone().into_bytes()),
        ("login token (string)", session.token.clone().into_bytes()),
        ("bob's token", bob.token.clone().into_bytes()),
    ];
    for token in [&alice.token, &session.token] {
        let raw = base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, token)
            .unwrap();
        secrets.push(("session token (bytes)", raw));
    }

    // positive control: the scan really does find plaintext when it is there
    sqlx::query("INSERT INTO meta (key, value) VALUES ('control', ?1)")
        .bind(b"CONTROL-PLAINTEXT-d41d8cd9".as_slice())
        .execute(&app.state.db)
        .await
        .unwrap();

    // 1) while the data is still in the write-ahead log
    let wal_phase = read_db_files(&app);
    assert!(
        contains(&wal_phase, b"CONTROL-PLAINTEXT-d41d8cd9"),
        "the scan is blind: a deliberately plaintext value was not found"
    );
    // 2) after being checkpointed into the main database file
    app.checkpoint().await;
    let db_phase = read_db_files(&app);
    assert!(contains(&db_phase, b"CONTROL-PLAINTEXT-d41d8cd9"));

    for (phase, bytes) in [("wal", &wal_phase), ("db", &db_phase)] {
        assert!(
            bytes.len() > 4096,
            "{phase}: suspiciously small ({})",
            bytes.len()
        );
        // the scan covers the rows that matter: the ciphertexts are in there
        assert!(
            contains(bytes, &share_req.payload_ct),
            "{phase}: the share ciphertext is not in the scanned bytes"
        );
        assert!(
            contains(bytes, &req.payload_ct),
            "{phase}: the item ciphertext is not in the scanned bytes"
        );
        for (what, secret) in &secrets {
            for (form, needle) in spellings(secret) {
                assert!(
                    !contains(bytes, &needle),
                    "{what} ({form}) found in the {phase} bytes of the database"
                );
            }
        }
    }

    // what the server does keep: hashes, not the keys themselves
    let (auth_hash, recovery_hash): (Vec<u8>, Vec<u8>) =
        sqlx::query_as("SELECT auth_hash, recovery_hash FROM users WHERE username = 'alice'")
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(auth_hash.len(), 32);
    assert_ne!(auth_hash, alice.auth_key.to_vec());
    assert_ne!(recovery_hash, alice.recovery_auth_key.to_vec());
}

// --- the landing page ---------------------------------------------------------------

fn assert_hardened(r: &Resp) {
    assert_eq!(r.header("referrer-policy"), Some("no-referrer"));
    assert_eq!(r.header("x-content-type-options"), Some("nosniff"));
    let csp = r.header("content-security-policy").expect("CSP");
    for directive in [
        "default-src 'none'",
        "script-src 'self'",
        "style-src 'self' 'unsafe-inline'",
        "img-src 'self'",
        "frame-ancestors 'none'",
    ] {
        assert!(csp.contains(directive), "CSP lacks {directive}: {csp}");
    }
    assert!(
        !csp.contains("connect-src"),
        "no network access for the page"
    );
    assert!(r.headers.get("set-cookie").is_none());
}

#[tokio::test]
async fn landing_page_is_static_hardened_and_does_not_leak_existence() {
    let app =
        TestApp::with_config(|c| c.public_url = Some("https://vault.example.test".into())).await;
    let alice = app.register("alice", PW).await;
    let (req, key) = share_request("x", None, None);
    let created: dotvault_core::api::CreateShareResponse = app
        .post("/v1/shares", Some(&alice.token), &req)
        .await
        .json();

    let real = app.get(&format!("/s/{}", created.id), None).await;
    let unknown = app.get("/s/AAAAAAAAAAAAAAAAAAAAAA", None).await;
    let junk = app.get("/s/not-even-an-id", None).await;
    for r in [&real, &unknown, &junk] {
        assert_eq!(r.status, StatusCode::OK);
        assert!(r.header("content-type").unwrap().starts_with("text/html"));
        assert_eq!(r.header("cache-control"), Some("no-store"));
        assert_hardened(r);
    }
    // identical bytes for an existing, an unknown and a malformed id
    assert_eq!(real.body, unknown.body);
    assert_eq!(real.body, junk.body);

    let html = real.text();
    assert!(html.contains("/assets/share.js"));
    assert!(
        html.contains("https://vault.example.test"),
        "public url is shown"
    );
    assert!(html.contains("brew install"), "install instructions");
    assert!(
        !html.contains(&created.id),
        "the id is not templated into the page"
    );
    assert!(!html.contains(&b64(&key[..])));
    // no external resources
    assert!(!html.contains("src=\"http") && !html.contains("href=\"http"));
    assert!(!html.contains("<link") && !html.contains("@import"));

    // the script
    let js = app.get("/assets/share.js", None).await;
    assert_eq!(js.status, StatusCode::OK);
    assert!(js.header("content-type").unwrap().contains("javascript"));
    assert_hardened(&js);
    let script = js.text();
    assert!(script.contains("location.hash"));
    assert!(script.contains("dotvault open dv1."));
    for network in [
        "fetch(",
        "XMLHttpRequest",
        "sendBeacon",
        "WebSocket",
        "new Image",
        "importScripts",
    ] {
        assert!(!script.contains(network), "script must not use {network}");
    }

    // only GET (and HEAD)
    let post = app.send(Req::new("POST", "/s/abc")).await;
    assert_eq!(post.status, StatusCode::METHOD_NOT_ALLOWED);
    let head = app
        .send(Req::new("HEAD", &format!("/s/{}", created.id)))
        .await;
    assert_eq!(head.status, StatusCode::OK);
    assert!(head.body.is_empty());
}

#[tokio::test]
async fn healthz() {
    let app = TestApp::new().await;
    let r = app.get("/healthz", None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.text(), "ok");
}

// --- over a real socket -----------------------------------------------------------------

async fn http(addr: std::net::SocketAddr, request: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut raw))
        .await
        .expect("response timed out")
        .unwrap();
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    (status, body)
}

fn get(path: &str, extra_headers: &str) -> String {
    format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{extra_headers}\r\n")
}

#[tokio::test]
async fn ephemeral_port_server_with_in_memory_db() {
    let config = Config {
        db: ":memory:".into(),
        auth_rate: RateLimit::per_minute(2),
        ..Config::default()
    };
    let server = harness::spawn(config).await.unwrap();
    assert!(server.base_url().starts_with("http://127.0.0.1:"));

    let (status, body) = http(server.addr, &get("/healthz", "")).await;
    assert_eq!((status, body.as_str()), (200, "ok"));

    // the client address comes from the real socket: two prelogins pass...
    for _ in 0..2 {
        let (status, body) = http(
            server.addr,
            &get(
                "/v1/auth/prelogin?username=ghost",
                "X-Forwarded-For: 203.0.113.50\r\n",
            ),
        )
        .await;
        assert_eq!(status, 200, "{body}");
        assert!(body.contains("kdf_salt"));
    }
    // ...and the third is limited, whatever X-Forwarded-For claims
    let (status, body) = http(
        server.addr,
        &get(
            "/v1/auth/prelogin?username=ghost",
            "X-Forwarded-For: 203.0.113.51\r\n",
        ),
    )
    .await;
    assert_eq!(status, 429, "{body}");
    assert!(body.contains("rate_limited"));

    // graceful shutdown completes and the port closes
    let addr = server.addr;
    tokio::time::timeout(Duration::from_secs(10), server.shutdown())
        .await
        .expect("graceful shutdown hung");
    assert!(TcpStream::connect(addr).await.is_err());
}

#[tokio::test]
async fn full_flow_over_a_real_socket_with_a_file_database() {
    // A tiny end-to-end pass through the same code path the CLI will use.
    let dir = tempfile::TempDir::new().unwrap();
    let config = Config {
        db: dir.path().join("e2e.db").to_string_lossy().into_owned(),
        auth_rate: RateLimit::per_minute(1000),
        ..Config::default()
    };
    let server = harness::spawn(config).await.unwrap();

    let acct = dotvault_core::crypto::create_account(PW, FAST).unwrap();
    let body = serde_json::to_string(&RegisterRequest {
        username: "dave".into(),
        kdf_salt: acct.password.kdf_salt.to_vec(),
        kdf_params: acct.password.kdf_params,
        auth_key: acct.password.auth_key.to_vec(),
        wrapped_vault_key: acct.password.wrapped_vault_key.clone(),
        recovery_auth_key: acct.recovery_auth_key.to_vec(),
        wrapped_vault_key_recovery: acct.wrapped_vault_key_recovery.clone(),
    })
    .unwrap();
    let request = format!(
        "POST /v1/auth/register HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let (status, resp) = http(server.addr, &request).await;
    assert_eq!(status, 200, "{resp}");
    let session: dotvault_core::api::SessionResponse = serde_json::from_str(&resp).unwrap();

    let (status, resp) = http(
        server.addr,
        &get(
            "/v1/me",
            &format!("Authorization: Bearer {}\r\n", session.token),
        ),
    )
    .await;
    assert_eq!(status, 200, "{resp}");
    assert!(resp.contains("\"dave\""));

    server.shutdown().await;
}

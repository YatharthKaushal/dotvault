//! The server must never log request bodies, tokens, keys or ciphertext.
//!
//! This binary holds a single test on purpose: it installs a *global*
//! tracing subscriber (so output from every thread is captured), which would
//! otherwise capture other tests' logs too.

mod common;

use std::io;
use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use common::*;
use dotvault_core::api::ReportEventRequest;
use dotvault_core::api::ShareAction;
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Capture {
    type Writer = Capture;
    fn make_writer(&'a self) -> Capture {
        self.clone()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn logs_never_contain_secrets_or_payloads() {
    const MARKER: &str = "LOGMARK-3f9a61c2d8";
    let capture = Capture::default();
    tracing_subscriber::fmt()
        .with_writer(capture.clone())
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .init();

    let app = TestApp::with_config(|c| {
        c.auth_rate = dotvault_server::config::RateLimit::per_minute(1000);
        c.share_open_rate = dotvault_server::config::RateLimit::per_minute(3);
    })
    .await;

    // a representative session of activity, including failures
    let password = format!("pw-{MARKER}");
    let alice = app.register("alice", &password).await;
    let bob = app.register("bob", "bob's password").await;
    let login = app.login("alice", &password).await;
    app.try_login("alice", "wrong guess").await;
    app.try_login("nobody-here", "x").await;

    let id = new_item_id();
    let item = create_item_request(
        &alice.vault_key,
        &id,
        &format!("name-{MARKER}"),
        &format!("K={MARKER}"),
    );
    app.post("/v1/vault/items", Some(&alice.token), &item).await;
    app.get(&format!("/v1/vault/items/{id}"), Some(&alice.token))
        .await;
    app.get("/v1/vault/items/doesnotexistdoesnotexi", Some(&bob.token))
        .await;

    let (share, share_key) = share_request(&format!("S={MARKER}"), None, Some(2));
    let created: dotvault_core::api::CreateShareResponse = app
        .post("/v1/shares", Some(&alice.token), &share)
        .await
        .json();
    for _ in 0..4 {
        app.post(
            &format!("/v1/shares/{}/open", created.id),
            Some(&bob.token),
            &serde_json::json!({}),
        )
        .await; // the last ones are 410 and 429
    }
    app.post(
        &format!("/v1/shares/{}/events", created.id),
        Some(&bob.token),
        &ReportEventRequest {
            action: ShareAction::Save,
        },
    )
    .await;
    app.delete(&format!("/v1/shares/{}", created.id), Some(&alice.token))
        .await;

    // malformed and hostile requests that carry the marker
    let junk = format!(r#"{{"username":"{MARKER}","auth_key":"{MARKER}!!"#);
    let r = app
        .send(Req::new("POST", "/v1/auth/login").raw(junk.into_bytes(), "application/json"))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    app.get(&format!("/v1/auth/prelogin?username=zz{MARKER}"), None)
        .await;
    app.get(&format!("/nope/{MARKER}"), None).await;
    app.get("/v1/me", Some(&format!("bad-token-{MARKER}")))
        .await;
    app.post(
        "/v1/auth/logout",
        Some(&login.token),
        &serde_json::json!({}),
    )
    .await;

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let logs = String::from_utf8_lossy(&capture.0.lock().unwrap()).into_owned();

    // the capture works and our own request log is present
    assert!(logs.contains("request"), "nothing was captured:\n{logs}");
    assert!(
        logs.contains("/v1/shares/{id}/open"),
        "route templates are logged"
    );
    assert!(logs.contains("rate limit exceeded"));

    // things that must not appear anywhere in the log
    let mut forbidden: Vec<(&str, String)> = vec![
        ("marker", MARKER.to_string()),
        ("password", password.clone()),
        ("alice's token", alice.token.clone()),
        ("login token", login.token.clone()),
        ("bob's token", bob.token.clone()),
        ("auth key", b64(&alice.auth_key[..])),
        ("recovery auth key", b64(&alice.recovery_auth_key[..])),
        ("recovery key", alice.recovery_key_text.clone()),
        ("share key", b64(&share_key[..])),
        ("share ciphertext", b64(&share.payload_ct)),
        ("item ciphertext", b64(&item.payload_ct)),
        ("item name ciphertext", b64(&item.name_ct)),
        ("share id", created.id.clone()),
        ("item id", id.clone()),
    ];
    forbidden.push(("vault key", b64(&alice.vault_key[..])));
    for (what, needle) in forbidden {
        assert!(
            !logs.contains(&needle),
            "{what} leaked into the logs:\n{logs}"
        );
    }
}

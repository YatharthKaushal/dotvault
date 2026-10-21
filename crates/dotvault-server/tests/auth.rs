mod common;

use axum::http::StatusCode;
use common::*;
use dotvault_core::{
    api::{
        ChangePasswordRequest, MeResponse, PreloginResponse, RecoverRequest, RecoverUnlockRequest,
        RecoverUnlockResponse, RegisterRequest,
    },
    crypto::{
        create_account, derive_account_keys, rewrap_for_password, unlock_with_recovery, KdfParams,
    },
};
use dotvault_server::config::SESSION_TTL_SECS;
use sha2::{Digest, Sha256};

const PW: &str = "correct horse battery staple";

#[tokio::test]
async fn register_login_me_logout() {
    let app = TestApp::new().await;
    let alice = app.register("alice", PW).await;

    // registration logs the client in
    let me = app.get("/v1/me", Some(&alice.token)).await;
    assert_eq!(me.status, StatusCode::OK);
    let me: MeResponse = me.json();
    assert_eq!(me.username, "alice");
    assert!((me.created_at - now()).abs() < 10);

    // a fresh login yields the same vault key, unwrapped locally
    let session = app.login("alice", PW).await;
    assert_eq!(*session.vault_key, *alice.vault_key);
    assert_ne!(session.token, alice.token);
    let ttl = session.expires_at - now();
    assert!(
        (SESSION_TTL_SECS - 60..=SESSION_TTL_SECS).contains(&ttl),
        "ttl {ttl}"
    );
    assert_eq!(
        app.get("/v1/me", Some(&session.token)).await.status,
        StatusCode::OK
    );

    // logout revokes exactly this session
    let out = app
        .post(
            "/v1/auth/logout",
            Some(&session.token),
            &serde_json::json!({}),
        )
        .await;
    assert_eq!(out.status, StatusCode::NO_CONTENT);
    app.get("/v1/me", Some(&session.token))
        .await
        .assert_error(StatusCode::UNAUTHORIZED, "unauthorized");
    assert_eq!(
        app.get("/v1/me", Some(&alice.token)).await.status,
        StatusCode::OK,
        "the other session must survive"
    );
}

#[tokio::test]
async fn sessions_store_only_a_hash_of_the_token() {
    let app = TestApp::new().await;
    let alice = app.register("alice", PW).await;

    let stored: Vec<u8> = sqlx::query_scalar("SELECT token_hash FROM sessions")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    let raw = base64::Engine::decode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        &alice.token,
    )
    .unwrap();
    assert_eq!(raw.len(), 32);
    assert_eq!(stored, Sha256::digest(&raw).to_vec());
    assert_ne!(stored, raw);
}

#[tokio::test]
async fn username_taken_and_case_insensitive() {
    let app = TestApp::new().await;
    app.register("Alice", PW).await;

    for dup in ["alice", "ALICE", "aLiCe"] {
        let resp = app.try_register(dup, "another password").await.unwrap_err();
        resp.assert_error(StatusCode::CONFLICT, "username_taken");
    }
    assert_eq!(app.count("users").await, 1);

    // any casing logs in, and the canonical name is lowercase
    let session = app.login("ALICE", PW).await;
    let me: MeResponse = app.get("/v1/me", Some(&session.token)).await.json();
    assert_eq!(me.username, "alice");
}

#[tokio::test]
async fn invalid_usernames_are_rejected() {
    let app = TestApp::new().await;
    for bad in [
        "ab",
        "has space",
        "semi;colon",
        "caf\u{e9}",
        &"x".repeat(33),
        "",
    ] {
        let resp = app.try_register(bad, PW).await.unwrap_err();
        resp.assert_error(StatusCode::BAD_REQUEST, "bad_request");
    }
    for good in ["abc", "a.b-c_d", &"x".repeat(32)] {
        app.register(good, PW).await;
    }
}

#[tokio::test]
async fn registration_validates_every_field() {
    let app = TestApp::new().await;
    let acct = create_account(PW, FAST).unwrap();
    let valid = RegisterRequest {
        username: "carol".into(),
        kdf_salt: acct.password.kdf_salt.to_vec(),
        kdf_params: acct.password.kdf_params,
        auth_key: acct.password.auth_key.to_vec(),
        wrapped_vault_key: acct.password.wrapped_vault_key.clone(),
        recovery_auth_key: acct.recovery_auth_key.to_vec(),
        wrapped_vault_key_recovery: acct.wrapped_vault_key_recovery.clone(),
    };

    let mut cases: Vec<(&str, RegisterRequest)> = Vec::new();
    let mut r = valid.clone();
    r.kdf_salt.pop();
    cases.push(("salt too short", r));
    let mut r = valid.clone();
    r.auth_key.push(0);
    cases.push(("auth_key too long", r));
    let mut r = valid.clone();
    r.recovery_auth_key.truncate(31);
    cases.push(("recovery_auth_key too short", r));
    let mut r = valid.clone();
    r.wrapped_vault_key = vec![0; 257];
    cases.push(("wrapped key too long", r));
    let mut r = valid.clone();
    r.wrapped_vault_key_recovery = vec![0; 257];
    cases.push(("recovery wrapped key too long", r));
    let mut r = valid.clone();
    r.wrapped_vault_key.clear();
    cases.push(("wrapped key empty", r));
    let mut r = valid.clone();
    r.kdf_params = KdfParams {
        m_kib: 1024,
        t: 1,
        p: 1,
    };
    cases.push(("weak kdf", r));
    let mut r = valid.clone();
    r.kdf_params = KdfParams {
        m_kib: 64 * 1024 * 1024,
        ..FAST
    };
    cases.push(("absurd kdf memory", r));

    for (why, req) in cases {
        let resp = app.post("/v1/auth/register", None, &req).await;
        assert_eq!(
            resp.status,
            StatusCode::BAD_REQUEST,
            "{why}: {}",
            resp.text()
        );
        assert_eq!(resp.error().code, "bad_request", "{why}");
    }
    assert_eq!(app.count("users").await, 0, "nothing was stored");

    // and the untouched request is fine
    assert_eq!(
        app.post("/v1/auth/register", None, &valid).await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn wrong_password_and_unknown_user_are_indistinguishable() {
    let app = TestApp::new().await;
    app.register("alice", PW).await;

    let wrong_password = app.try_login("alice", "not the password").await;
    let unknown_user = app.try_login("nobody", PW).await;

    wrong_password.assert_error(StatusCode::UNAUTHORIZED, "invalid_credentials");
    unknown_user.assert_error(StatusCode::UNAUTHORIZED, "invalid_credentials");
    assert_eq!(wrong_password.status, unknown_user.status);
    assert_eq!(
        wrong_password.body, unknown_user.body,
        "bodies must be identical"
    );
    assert_eq!(
        wrong_password.header("content-type"),
        unknown_user.header("content-type")
    );
}

#[tokio::test]
async fn prelogin_fake_salt_is_stable_and_per_username() {
    let app = TestApp::new().await;
    app.register("alice", PW).await;

    let pre = |name: &str| {
        let path = format!("/v1/auth/prelogin?username={name}");
        let app = &app;
        async move {
            let r = app.get(&path, None).await;
            assert_eq!(r.status, StatusCode::OK);
            r.json::<PreloginResponse>()
        }
    };

    let ghost1 = pre("ghost").await;
    let ghost2 = pre("ghost").await;
    assert_eq!(ghost1.kdf_salt, ghost2.kdf_salt, "stable across calls");
    assert_eq!(ghost1.kdf_salt.len(), 16);
    assert_eq!(ghost1.kdf_params, KdfParams::default());
    assert_eq!(
        pre("GHOST").await.kdf_salt,
        ghost1.kdf_salt,
        "case-insensitive"
    );

    let other = pre("phantom").await;
    assert_ne!(other.kdf_salt, ghost1.kdf_salt, "differs per username");

    // the real user gets the real salt and parameters
    let alice = pre("alice").await;
    assert_eq!(alice.kdf_params, FAST);
    assert_ne!(alice.kdf_salt, ghost1.kdf_salt);
    let stored: Vec<u8> = sqlx::query_scalar("SELECT kdf_salt FROM users WHERE username = 'alice'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(alice.kdf_salt, stored);

    // missing parameter is a client error
    app.get("/v1/auth/prelogin", None)
        .await
        .assert_error(StatusCode::BAD_REQUEST, "bad_request");

    // unusual usernames are answered like any other unknown name
    for odd in ["a", "with%20space", "%F0%9F%98%80"] {
        let r = app
            .get(&format!("/v1/auth/prelogin?username={odd}"), None)
            .await;
        assert_eq!(r.status, StatusCode::OK, "{odd}");
    }
}

#[tokio::test]
async fn fake_salt_survives_a_restart_and_follows_the_configured_secret() {
    let app = TestApp::new().await;
    let before: PreloginResponse = app
        .get("/v1/auth/prelogin?username=ghost", None)
        .await
        .json();

    // no DOTVAULT_SERVER_SECRET: the generated one is persisted in `meta`
    let restarted = app.restarted().await;
    let after = send_to(
        &restarted,
        Req::new("GET", "/v1/auth/prelogin?username=ghost"),
    )
    .await;
    assert_eq!(after.json::<PreloginResponse>().kdf_salt, before.kdf_salt);

    // an operator-supplied secret takes over
    let configured =
        TestApp::with_config(|c| c.server_secret = Some("an-operator-chosen-secret-value".into()))
            .await;
    let a: PreloginResponse = configured
        .get("/v1/auth/prelogin?username=ghost", None)
        .await
        .json();
    assert_ne!(a.kdf_salt, before.kdf_salt);
    let same_secret =
        TestApp::with_config(|c| c.server_secret = Some("an-operator-chosen-secret-value".into()))
            .await;
    let b: PreloginResponse = same_secret
        .get("/v1/auth/prelogin?username=ghost", None)
        .await
        .json();
    assert_eq!(a.kdf_salt, b.kdf_salt);
}

#[tokio::test]
async fn password_change_revokes_other_sessions_and_keeps_the_vault_key() {
    let app = TestApp::new().await;
    let alice = app.register("alice", PW).await; // session A
    let other_device = app.login("alice", PW).await; // session B

    let new_pw = "a brand new passphrase";
    let material = rewrap_for_password(new_pw, FAST, &alice.vault_key).unwrap();
    let change = ChangePasswordRequest {
        current_auth_key: alice.auth_key.to_vec(),
        new_kdf_salt: material.kdf_salt.to_vec(),
        new_kdf_params: material.kdf_params,
        new_auth_key: material.auth_key.to_vec(),
        new_wrapped_vault_key: material.wrapped_vault_key.clone(),
    };

    // a wrong current password changes nothing
    let mut bad = change.clone();
    bad.current_auth_key = vec![7; 32];
    app.post("/v1/auth/password", Some(&alice.token), &bad)
        .await
        .assert_error(StatusCode::UNAUTHORIZED, "invalid_credentials");
    assert_eq!(app.try_login("alice", PW).await.status, StatusCode::OK);

    // the real change
    let resp = app
        .post("/v1/auth/password", Some(&alice.token), &change)
        .await;
    assert_eq!(resp.status, StatusCode::NO_CONTENT, "{}", resp.text());

    // the calling session lives on, every other one is gone
    assert_eq!(
        app.get("/v1/me", Some(&alice.token)).await.status,
        StatusCode::OK
    );
    app.get("/v1/me", Some(&other_device.token))
        .await
        .assert_error(StatusCode::UNAUTHORIZED, "unauthorized");

    // old password no longer works, new one does and unwraps the same key
    app.try_login("alice", PW)
        .await
        .assert_error(StatusCode::UNAUTHORIZED, "invalid_credentials");
    let session = app.login("alice", new_pw).await;
    assert_eq!(*session.vault_key, *alice.vault_key);

    // salt and parameters were replaced too
    let pre: PreloginResponse = app
        .get("/v1/auth/prelogin?username=alice", None)
        .await
        .json();
    assert_eq!(pre.kdf_salt, material.kdf_salt.to_vec());
}

#[tokio::test]
async fn password_change_validates_new_material() {
    let app = TestApp::new().await;
    let alice = app.register("alice", PW).await;
    let material = rewrap_for_password("whatever", FAST, &alice.vault_key).unwrap();
    let good = ChangePasswordRequest {
        current_auth_key: alice.auth_key.to_vec(),
        new_kdf_salt: material.kdf_salt.to_vec(),
        new_kdf_params: material.kdf_params,
        new_auth_key: material.auth_key.to_vec(),
        new_wrapped_vault_key: material.wrapped_vault_key.clone(),
    };
    let mut weak = good.clone();
    weak.new_kdf_params = KdfParams {
        m_kib: 8,
        t: 1,
        p: 1,
    };
    let mut short_salt = good.clone();
    short_salt.new_kdf_salt.pop();
    for bad in [weak, short_salt] {
        app.post("/v1/auth/password", Some(&alice.token), &bad)
            .await
            .assert_error(StatusCode::BAD_REQUEST, "bad_request");
    }
    // still the old password
    assert_eq!(app.try_login("alice", PW).await.status, StatusCode::OK);
}

#[tokio::test]
async fn full_recovery_flow() {
    let app = TestApp::new().await;
    let alice = app.register("alice", PW).await;
    let second_device = app.login("alice", PW).await;

    // The user types the recovery key they were shown back in.
    let (_, recovery) = recovery_auth_key_from_text(&alice.recovery_key_text);

    // step 1: prove possession, receive the vault key wrapped under it
    let unlock = app
        .post(
            "/v1/auth/recover/unlock",
            None,
            &RecoverUnlockRequest {
                username: "Alice".into(), // casing is irrelevant
                recovery_auth_key: recovery.auth_key.to_vec(),
            },
        )
        .await;
    assert_eq!(unlock.status, StatusCode::OK, "{}", unlock.text());
    let unlock: RecoverUnlockResponse = unlock.json();
    let vault_key = unlock_with_recovery(&recovery, &unlock.wrapped_vault_key_recovery).unwrap();
    assert_eq!(*vault_key, *alice.vault_key);

    // step 2: choose a new password for the recovered vault key
    let new_pw = "fresh start 2025";
    let material = rewrap_for_password(new_pw, FAST, &vault_key).unwrap();
    let resp = app
        .post(
            "/v1/auth/recover",
            None,
            &RecoverRequest {
                username: "alice".into(),
                recovery_auth_key: recovery.auth_key.to_vec(),
                new_kdf_salt: material.kdf_salt.to_vec(),
                new_kdf_params: material.kdf_params,
                new_auth_key: material.auth_key.to_vec(),
                new_wrapped_vault_key: material.wrapped_vault_key.clone(),
            },
        )
        .await;
    assert_eq!(resp.status, StatusCode::NO_CONTENT, "{}", resp.text());

    // every session is gone, including the one that could have been stolen
    for token in [&alice.token, &second_device.token] {
        app.get("/v1/me", Some(token))
            .await
            .assert_error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    assert_eq!(app.count("sessions").await, 0);

    // old password is dead, the new one opens the very same vault key
    app.try_login("alice", PW)
        .await
        .assert_error(StatusCode::UNAUTHORIZED, "invalid_credentials");
    let session = app.login("alice", new_pw).await;
    assert_eq!(*session.vault_key, *alice.vault_key);

    // the recovery key keeps working (it was not consumed)
    let again = app
        .post(
            "/v1/auth/recover/unlock",
            None,
            &RecoverUnlockRequest {
                username: "alice".into(),
                recovery_auth_key: recovery.auth_key.to_vec(),
            },
        )
        .await;
    assert_eq!(again.status, StatusCode::OK);
}

#[tokio::test]
async fn recovery_failures_are_generic() {
    let app = TestApp::new().await;
    let alice = app.register("alice", PW).await;
    let material = rewrap_for_password("x", FAST, &alice.vault_key).unwrap();
    let wrong_key = vec![9u8; 32];

    let unlock = |username: &str, key: Vec<u8>| RecoverUnlockRequest {
        username: username.to_string(),
        recovery_auth_key: key,
    };
    let recover = |username: &str, key: Vec<u8>| RecoverRequest {
        username: username.to_string(),
        recovery_auth_key: key,
        new_kdf_salt: material.kdf_salt.to_vec(),
        new_kdf_params: material.kdf_params,
        new_auth_key: material.auth_key.to_vec(),
        new_wrapped_vault_key: material.wrapped_vault_key.clone(),
    };

    let bad_key = app
        .post(
            "/v1/auth/recover/unlock",
            None,
            &unlock("alice", wrong_key.clone()),
        )
        .await;
    let no_user = app
        .post(
            "/v1/auth/recover/unlock",
            None,
            &unlock("nobody", wrong_key.clone()),
        )
        .await;
    bad_key.assert_error(StatusCode::UNAUTHORIZED, "invalid_credentials");
    assert_eq!(bad_key.body, no_user.body);
    assert_eq!(bad_key.status, no_user.status);

    let bad_key = app
        .post(
            "/v1/auth/recover",
            None,
            &recover("alice", wrong_key.clone()),
        )
        .await;
    let no_user = app
        .post(
            "/v1/auth/recover",
            None,
            &recover("nobody", wrong_key.clone()),
        )
        .await;
    bad_key.assert_error(StatusCode::UNAUTHORIZED, "invalid_credentials");
    assert_eq!(bad_key.body, no_user.body);

    // the account is untouched: password and session still work
    assert_eq!(app.try_login("alice", PW).await.status, StatusCode::OK);
    assert_eq!(
        app.get("/v1/me", Some(&alice.token)).await.status,
        StatusCode::OK
    );

    // the password-derived auth key is not a recovery key
    let resp = app
        .post(
            "/v1/auth/recover/unlock",
            None,
            &unlock("alice", alice.auth_key.to_vec()),
        )
        .await;
    assert_eq!(resp.status, StatusCode::UNAUTHORIZED);

    // malformed length is a plain 400
    let resp = app
        .post(
            "/v1/auth/recover/unlock",
            None,
            &unlock("alice", vec![1; 5]),
        )
        .await;
    resp.assert_error(StatusCode::BAD_REQUEST, "bad_request");
}

#[tokio::test]
async fn sessions_slide_but_are_written_at_most_hourly() {
    let app = TestApp::new().await;
    let alice = app.register("alice", PW).await;
    let n = now();

    // used a minute ago: no write, values untouched
    sqlx::query("UPDATE sessions SET last_used_at = ?1, expires_at = ?2")
        .bind(n - 60)
        .bind(n + 1000)
        .execute(&app.state.db)
        .await
        .unwrap();
    assert_eq!(
        app.get("/v1/me", Some(&alice.token)).await.status,
        StatusCode::OK
    );
    let (last, exp): (i64, i64) = sqlx::query_as("SELECT last_used_at, expires_at FROM sessions")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!((last, exp), (n - 60, n + 1000));

    // last used two hours ago: bumped to a fresh 30 days
    sqlx::query("UPDATE sessions SET last_used_at = ?1, expires_at = ?2")
        .bind(n - 7200)
        .bind(n + 1000)
        .execute(&app.state.db)
        .await
        .unwrap();
    assert_eq!(
        app.get("/v1/me", Some(&alice.token)).await.status,
        StatusCode::OK
    );
    let (last, exp): (i64, i64) = sqlx::query_as("SELECT last_used_at, expires_at FROM sessions")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert!(last >= n && last <= now());
    assert!(exp >= n + SESSION_TTL_SECS && exp <= now() + SESSION_TTL_SECS);

    // an expired session is rejected and not revived
    sqlx::query("UPDATE sessions SET expires_at = ?1")
        .bind(n - 1)
        .execute(&app.state.db)
        .await
        .unwrap();
    app.get("/v1/me", Some(&alice.token))
        .await
        .assert_error(StatusCode::UNAUTHORIZED, "unauthorized");
    let exp: i64 = sqlx::query_scalar("SELECT expires_at FROM sessions")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(exp, n - 1);
}

#[tokio::test]
async fn device_name_is_optional_and_truncated() {
    let app = TestApp::new().await;
    app.register("alice", PW).await;

    let long = "d".repeat(100);
    let resp = app.try_login_as("alice", PW, Some(&long)).await;
    assert_eq!(resp.status, StatusCode::OK);
    let resp = app.try_login_as("alice", PW, Some("laptop")).await;
    assert_eq!(resp.status, StatusCode::OK);
    assert_eq!(app.try_login("alice", PW).await.status, StatusCode::OK);

    let names: Vec<Option<String>> =
        sqlx::query_scalar("SELECT device_name FROM sessions ORDER BY id")
            .fetch_all(&app.state.db)
            .await
            .unwrap();
    // [register (none), long (cut to 64), laptop, none]
    assert_eq!(names.len(), 4);
    assert_eq!(names[0], None);
    assert_eq!(names[1].as_deref().map(str::len), Some(64));
    assert_eq!(names[2].as_deref(), Some("laptop"));
    assert_eq!(names[3], None);
}

#[tokio::test]
async fn every_failed_request_has_the_json_error_shape() {
    let app = TestApp::new().await;

    // malformed JSON
    let r = app
        .send(Req::new("POST", "/v1/auth/login").raw(b"{not json".to_vec(), "application/json"))
        .await;
    r.assert_error(StatusCode::BAD_REQUEST, "bad_request");

    // missing fields
    let r = app
        .send(Req::new("POST", "/v1/auth/login").raw(b"{}".to_vec(), "application/json"))
        .await;
    r.assert_error(StatusCode::BAD_REQUEST, "bad_request");

    // bad base64 in a binary field
    let r = app
        .send(Req::new("POST", "/v1/auth/login").raw(
            br#"{"username":"a","auth_key":"not base64!!"}"#.to_vec(),
            "application/json",
        ))
        .await;
    r.assert_error(StatusCode::BAD_REQUEST, "bad_request");

    // wrong content type
    let r = app
        .send(Req::new("POST", "/v1/auth/login").raw(b"{}".to_vec(), "text/plain"))
        .await;
    r.assert_error(StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_media_type");

    // unknown route and wrong method
    app.get("/v1/nope", None)
        .await
        .assert_error(StatusCode::NOT_FOUND, "not_found");
    app.get("/nope", None)
        .await
        .assert_error(StatusCode::NOT_FOUND, "not_found");
    app.get("/v1/auth/login", None)
        .await
        .assert_error(StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed");

    // API responses are never cached or sniffed
    let r = app.get("/v1/me", None).await;
    assert_eq!(r.header("cache-control"), Some("no-store"));
    assert_eq!(r.header("x-content-type-options"), Some("nosniff"));
}

#[tokio::test]
async fn derive_keys_match_what_the_server_returns() {
    // The salt the server hands out must be the one that was registered, or
    // no client could ever log in.
    let app = TestApp::new().await;
    let alice = app.register("alice", PW).await;
    let pre: PreloginResponse = app
        .get("/v1/auth/prelogin?username=alice", None)
        .await
        .json();
    let keys = derive_account_keys(PW, &pre.kdf_salt, &pre.kdf_params).unwrap();
    assert_eq!(*keys.auth_key, *alice.auth_key);
}

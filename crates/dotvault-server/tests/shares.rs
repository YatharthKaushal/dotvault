mod common;

use axum::http::StatusCode;
use common::*;
use dotvault_core::{
    api::{
        CreateShareRequest, CreateShareResponse, ListSharesResponse, OpenShareResponse,
        ReportEventRequest, ShareAction, ShareEventsResponse,
    },
    crypto::{open, AAD_SHARE},
};
use dotvault_server::janitor;

const PW: &str = "correct horse battery staple";

struct World {
    app: TestApp,
    alice: Account,
    bob: Account,
}

async fn world() -> World {
    let app = TestApp::new().await;
    let alice = app.register("alice", PW).await;
    let bob = app.register("bob", PW).await;
    World { app, alice, bob }
}

impl World {
    async fn share(
        &self,
        owner: &Account,
        plaintext: &str,
        ttl: Option<u64>,
        max_views: Option<u32>,
    ) -> (CreateShareResponse, dotvault_core::crypto::Key) {
        let (req, key) = share_request(plaintext, ttl, max_views);
        let resp = self.app.post("/v1/shares", Some(&owner.token), &req).await;
        assert_eq!(resp.status, StatusCode::CREATED, "{}", resp.text());
        (resp.json(), key)
    }

    async fn open(&self, who: &Account, id: &str) -> Resp {
        self.app
            .send(
                Req::new("POST", &format!("/v1/shares/{id}/open"))
                    .token(&who.token)
                    .header("user-agent", "dotvault-cli/0.1.0 (test)"),
            )
            .await
    }

    async fn events(&self, who: &Account, id: &str) -> Resp {
        self.app
            .get(&format!("/v1/shares/{id}/events"), Some(&who.token))
            .await
    }

    async fn report(&self, who: &Account, id: &str, action: ShareAction) -> Resp {
        self.app
            .post(
                &format!("/v1/shares/{id}/events"),
                Some(&who.token),
                &ReportEventRequest { action },
            )
            .await
    }
}

#[tokio::test]
async fn create_and_open_roundtrip() {
    let w = world().await;
    let (created, key) = w
        .share(&w.alice, "DB_URL=postgres://shared", None, None)
        .await;
    assert_eq!(created.id.len(), 22);

    let resp = w.open(&w.bob, &created.id).await;
    assert_eq!(resp.status, StatusCode::OK, "{}", resp.text());
    let opened: OpenShareResponse = resp.json();
    assert_eq!(opened.id, created.id);
    assert_eq!(opened.owner, "alice");
    assert_eq!(opened.expires_at, created.expires_at);
    assert_eq!(opened.views_left, None);
    assert!((opened.created_at - now()).abs() < 10);
    let plain = open(&key, &opened.payload_ct, AAD_SHARE).unwrap();
    assert_eq!(&plain[..], b"DB_URL=postgres://shared");

    // the owner's view of it
    let list: ListSharesResponse = w.app.get("/v1/shares", Some(&w.alice.token)).await.json();
    assert_eq!(list.shares.len(), 1);
    let s = &list.shares[0];
    assert_eq!((s.id.as_str(), s.view_count), (created.id.as_str(), 1));
    assert_eq!((s.max_views, s.revoked, s.expired), (None, false, false));

    // the log shows who viewed, with the client string
    let events: ShareEventsResponse = w.events(&w.alice, &created.id).await.json();
    assert_eq!(events.events.len(), 1);
    let e = &events.events[0];
    assert_eq!(e.user, "bob");
    assert_eq!(e.action, ShareAction::View);
    assert_eq!(e.client.as_deref(), Some("dotvault-cli/0.1.0 (test)"));
    assert!((e.created_at - now()).abs() < 10);
}

#[tokio::test]
async fn ttl_and_limits_are_applied_and_clamped() {
    let w = world().await;
    let expiry = |r: &CreateShareResponse| r.expires_at - now();

    let (d, _) = w.share(&w.alice, "x", None, None).await;
    assert!((expiry(&d) - 24 * 3600).abs() <= 5, "default is 24h");
    let (short, _) = w.share(&w.alice, "x", Some(5), None).await;
    assert!((expiry(&short) - 60).abs() <= 5, "minimum is 60s");
    let (exact, _) = w.share(&w.alice, "x", Some(7200), None).await;
    assert!((expiry(&exact) - 7200).abs() <= 5);
    let (long, _) = w.share(&w.alice, "x", Some(u64::MAX), None).await;
    assert!(
        (expiry(&long) - 30 * 24 * 3600).abs() <= 5,
        "ceiling is 30 days"
    );

    // max_views must be 1..=1000
    let t = Some(w.alice.token.as_str());
    for bad in [0u32, 1001, u32::MAX] {
        let (req, _) = share_request("x", None, Some(bad));
        w.app
            .post("/v1/shares", t, &req)
            .await
            .assert_error(StatusCode::BAD_REQUEST, "bad_request");
    }
    for good in [1u32, 1000] {
        let (req, _) = share_request("x", None, Some(good));
        assert_eq!(
            w.app.post("/v1/shares", t, &req).await.status,
            StatusCode::CREATED
        );
    }

    // payload size: 64 KiB is the ceiling
    let mk = |len| CreateShareRequest {
        payload_ct: vec![9; len],
        ttl_secs: None,
        max_views: None,
    };
    assert_eq!(
        w.app.post("/v1/shares", t, &mk(64 * 1024)).await.status,
        StatusCode::CREATED
    );
    w.app
        .post("/v1/shares", t, &mk(64 * 1024 + 1))
        .await
        .assert_error(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large");
    w.app
        .post("/v1/shares", t, &mk(0))
        .await
        .assert_error(StatusCode::BAD_REQUEST, "bad_request");
}

#[tokio::test]
async fn open_requires_an_account() {
    let w = world().await;
    let (created, _) = w.share(&w.alice, "x", None, None).await;
    let path = format!("/v1/shares/{}/open", created.id);
    w.app
        .send(Req::new("POST", &path))
        .await
        .assert_error(StatusCode::UNAUTHORIZED, "unauthorized");
    w.app
        .send(Req::new("POST", &path).token("bm90IGEgcmVhbCB0b2tlbg"))
        .await
        .assert_error(StatusCode::UNAUTHORIZED, "unauthorized");
    // and nothing was counted
    let views: i64 = sqlx::query_scalar("SELECT view_count FROM shares")
        .fetch_one(&w.app.state.db)
        .await
        .unwrap();
    assert_eq!(views, 0);
    assert_eq!(w.app.count("share_events").await, 0);
}

#[tokio::test]
async fn max_views_is_enforced_then_410() {
    let w = world().await;
    let (created, _) = w.share(&w.alice, "x", None, Some(2)).await;

    let first: OpenShareResponse = w.open(&w.bob, &created.id).await.json();
    assert_eq!(first.views_left, Some(1));
    let second: OpenShareResponse = w.open(&w.bob, &created.id).await.json();
    assert_eq!(second.views_left, Some(0));

    // exhausted: gone, for the owner too
    w.open(&w.bob, &created.id)
        .await
        .assert_error(StatusCode::GONE, "share_gone");
    w.open(&w.alice, &created.id)
        .await
        .assert_error(StatusCode::GONE, "share_gone");

    // refused attempts are neither counted nor logged
    let list: ListSharesResponse = w.app.get("/v1/shares", Some(&w.alice.token)).await.json();
    assert_eq!(list.shares[0].view_count, 2);
    assert_eq!(list.shares[0].max_views, Some(2));
    let events: ShareEventsResponse = w.events(&w.alice, &created.id).await.json();
    assert_eq!(events.events.len(), 2);

    // an id that never existed is 404, not 410
    w.open(&w.bob, "AAAAAAAAAAAAAAAAAAAAAA")
        .await
        .assert_error(StatusCode::NOT_FOUND, "not_found");
    w.open(&w.bob, "garbage")
        .await
        .assert_error(StatusCode::NOT_FOUND, "not_found");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_opens_never_exceed_max_views() {
    let w = world().await;
    let (created, _) = w.share(&w.alice, "x", None, Some(3)).await;

    let mut tasks = Vec::new();
    for _ in 0..12 {
        let router = w.app.router.clone();
        let token = w.bob.token.clone();
        let path = format!("/v1/shares/{}/open", created.id);
        tasks.push(tokio::spawn(async move {
            send_to(&router, Req::new("POST", &path).token(&token))
                .await
                .status
        }));
    }
    let mut ok = 0;
    let mut gone = 0;
    for t in tasks {
        match t.await.unwrap() {
            StatusCode::OK => ok += 1,
            StatusCode::GONE => gone += 1,
            other => panic!("unexpected status {other}"),
        }
    }
    assert_eq!((ok, gone), (3, 9));
    let views: i64 = sqlx::query_scalar("SELECT view_count FROM shares")
        .fetch_one(&w.app.state.db)
        .await
        .unwrap();
    assert_eq!(views, 3);
    assert_eq!(w.app.count("share_events").await, 3);
}

#[tokio::test]
async fn expiry_is_enforced_and_the_janitor_purges_ciphertext() {
    let w = world().await;
    let (live, _) = w.share(&w.alice, "x", None, None).await;
    let (stale, _) = w.share(&w.alice, "y", None, None).await;
    let opened_before: OpenShareResponse = w.open(&w.bob, &stale.id).await.json();
    assert!(!opened_before.payload_ct.is_empty());

    // age the share by editing the row, as the clock cannot be moved
    sqlx::query("UPDATE shares SET expires_at = ?1 WHERE id = ?2")
        .bind(now() - 1)
        .bind(&stale.id)
        .execute(&w.app.state.db)
        .await
        .unwrap();

    w.open(&w.bob, &stale.id)
        .await
        .assert_error(StatusCode::GONE, "share_gone");
    assert_eq!(w.open(&w.bob, &live.id).await.status, StatusCode::OK);

    let list: ListSharesResponse = w.app.get("/v1/shares", Some(&w.alice.token)).await.json();
    let find = |id: &str| list.shares.iter().find(|s| s.id == id).unwrap();
    assert!(find(&stale.id).expired && !find(&stale.id).revoked);
    assert!(!find(&live.id).expired);

    // ciphertext is still there until the janitor runs...
    let present: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM shares WHERE payload_ct IS NOT NULL")
            .fetch_one(&w.app.state.db)
            .await
            .unwrap();
    assert_eq!(present, 2);

    // ...an expired session goes in the same pass
    sqlx::query("UPDATE sessions SET expires_at = ?1 WHERE user_id = ?2")
        .bind(now() - 10)
        .bind(w.app.user_id("bob").await)
        .execute(&w.app.state.db)
        .await
        .unwrap();
    let swept = janitor::run_once(&w.app.state.db, now()).await.unwrap();
    assert_eq!(swept.shares_purged, 1);
    assert_eq!(swept.sessions_deleted, 1);

    let nulled: Option<Vec<u8>> = sqlx::query_scalar("SELECT payload_ct FROM shares WHERE id = ?1")
        .bind(&stale.id)
        .fetch_one(&w.app.state.db)
        .await
        .unwrap();
    assert!(nulled.is_none(), "expired ciphertext must be purged");
    let kept: Option<Vec<u8>> = sqlx::query_scalar("SELECT payload_ct FROM shares WHERE id = ?1")
        .bind(&live.id)
        .fetch_one(&w.app.state.db)
        .await
        .unwrap();
    assert!(kept.is_some(), "live shares are untouched");

    // rows and the audit log survive the purge
    assert_eq!(
        w.events(&w.alice, &stale.id)
            .await
            .json::<ShareEventsResponse>()
            .events
            .len(),
        1
    );
    // bob's session expired, alice's did not
    w.app
        .get("/v1/me", Some(&w.bob.token))
        .await
        .assert_error(StatusCode::UNAUTHORIZED, "unauthorized");
    assert_eq!(
        w.app.get("/v1/me", Some(&w.alice.token)).await.status,
        StatusCode::OK
    );

    // a second pass has nothing left to do
    let again = janitor::run_once(&w.app.state.db, now()).await.unwrap();
    assert_eq!(again, janitor::Swept::default());
}

#[tokio::test]
async fn revoke_kills_the_share_and_nulls_the_ciphertext() {
    let w = world().await;
    let (created, _) = w.share(&w.alice, "SECRET=1", None, None).await;
    assert_eq!(w.open(&w.bob, &created.id).await.status, StatusCode::OK);

    let resp = w
        .app
        .delete(&format!("/v1/shares/{}", created.id), Some(&w.alice.token))
        .await;
    assert_eq!(resp.status, StatusCode::NO_CONTENT);

    w.open(&w.bob, &created.id)
        .await
        .assert_error(StatusCode::GONE, "share_gone");

    let (ct, revoked_at): (Option<Vec<u8>>, Option<i64>) =
        sqlx::query_as("SELECT payload_ct, revoked_at FROM shares WHERE id = ?1")
            .bind(&created.id)
            .fetch_one(&w.app.state.db)
            .await
            .unwrap();
    assert!(ct.is_none(), "ciphertext must be deleted on revoke");
    assert!(revoked_at.is_some());

    // the row and its audit trail remain visible to the owner
    let list: ListSharesResponse = w.app.get("/v1/shares", Some(&w.alice.token)).await.json();
    assert!(list.shares[0].revoked);
    let events: ShareEventsResponse = w.events(&w.alice, &created.id).await.json();
    assert_eq!(events.events.len(), 1);
    assert_eq!(events.events[0].user, "bob");

    // revoking again is harmless
    let again = w
        .app
        .delete(&format!("/v1/shares/{}", created.id), Some(&w.alice.token))
        .await;
    assert_eq!(again.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn only_the_owner_can_list_inspect_or_revoke() {
    let w = world().await;
    let carol = w.app.register("carol", PW).await;
    let (created, _) = w.share(&w.alice, "x", None, None).await;
    assert_eq!(w.open(&w.bob, &created.id).await.status, StatusCode::OK);
    let path = format!("/v1/shares/{}", created.id);

    // other users get 404, identical to a share that does not exist
    let missing = w.events(&carol, "AAAAAAAAAAAAAAAAAAAAAA").await;
    for who in [&w.bob, &carol] {
        let r = w.events(who, &created.id).await;
        r.assert_error(StatusCode::NOT_FOUND, "not_found");
        assert_eq!(r.body, missing.body);
        let r = w.app.delete(&path, Some(&who.token)).await;
        r.assert_error(StatusCode::NOT_FOUND, "not_found");
        let l: ListSharesResponse = w.app.get("/v1/shares", Some(&who.token)).await.json();
        assert!(l.shares.is_empty(), "listing is per owner");
    }

    // the failed revoke attempts did nothing
    assert_eq!(w.open(&w.bob, &created.id).await.status, StatusCode::OK);
    let revoked: Option<i64> = sqlx::query_scalar("SELECT revoked_at FROM shares")
        .fetch_one(&w.app.state.db)
        .await
        .unwrap();
    assert!(revoked.is_none());

    // unauthenticated is 401 everywhere
    for (m, p) in [
        ("GET", "/v1/shares".to_string()),
        ("DELETE", path.clone()),
        ("GET", format!("{path}/events")),
        ("POST", format!("{path}/events")),
    ] {
        w.app
            .send(Req::new(m, &p))
            .await
            .assert_error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
}

#[tokio::test]
async fn save_events_require_a_prior_view() {
    let w = world().await;
    let (created, _) = w.share(&w.alice, "x", None, None).await;
    let id = &created.id;

    // bob has not opened it yet; the owner never opened it either
    w.report(&w.bob, id, ShareAction::Save)
        .await
        .assert_error(StatusCode::FORBIDDEN, "forbidden");
    w.report(&w.alice, id, ShareAction::Save)
        .await
        .assert_error(StatusCode::FORBIDDEN, "forbidden");
    assert_eq!(w.app.count("share_events").await, 0);

    // clients cannot forge views
    w.report(&w.bob, id, ShareAction::View)
        .await
        .assert_error(StatusCode::BAD_REQUEST, "bad_request");
    assert_eq!(w.app.count("share_events").await, 0);

    // unknown share
    w.report(&w.bob, "AAAAAAAAAAAAAAAAAAAAAA", ShareAction::Save)
        .await
        .assert_error(StatusCode::NOT_FOUND, "not_found");

    // after opening, bob may report a save
    assert_eq!(w.open(&w.bob, id).await.status, StatusCode::OK);
    let resp = w
        .app
        .send(
            Req::new("POST", &format!("/v1/shares/{id}/events"))
                .token(&w.bob.token)
                .header("user-agent", "dotvault-cli/0.1.0 (test)")
                .json(&ReportEventRequest {
                    action: ShareAction::Save,
                }),
        )
        .await;
    assert_eq!(resp.status, StatusCode::NO_CONTENT, "{}", resp.text());

    // someone else's view does not unlock saving for carol
    let carol = w.app.register("carol", PW).await;
    w.report(&carol, id, ShareAction::Save)
        .await
        .assert_error(StatusCode::FORBIDDEN, "forbidden");

    // the owner sees who did what, oldest first
    let events: ShareEventsResponse = w.events(&w.alice, id).await.json();
    let summary: Vec<(String, ShareAction)> = events
        .events
        .iter()
        .map(|e| (e.user.clone(), e.action))
        .collect();
    assert_eq!(
        summary,
        vec![
            ("bob".to_string(), ShareAction::View),
            ("bob".to_string(), ShareAction::Save)
        ]
    );
    assert!(events.events[0].created_at <= events.events[1].created_at);
    assert_eq!(
        events.events[1].client.as_deref(),
        Some("dotvault-cli/0.1.0 (test)")
    );

    // view_count counts opens only, not saves
    let list: ListSharesResponse = w.app.get("/v1/shares", Some(&w.alice.token)).await.json();
    assert_eq!(list.shares[0].view_count, 1);
}

#[tokio::test]
async fn event_log_names_every_viewer() {
    let w = world().await;
    let carol = w.app.register("carol", PW).await;
    let (created, _) = w.share(&w.alice, "x", None, None).await;

    for who in [&w.bob, &carol, &w.bob] {
        assert_eq!(w.open(who, &created.id).await.status, StatusCode::OK);
    }
    let events: ShareEventsResponse = w.events(&w.alice, &created.id).await.json();
    let who: Vec<&str> = events.events.iter().map(|e| e.user.as_str()).collect();
    assert_eq!(who, ["bob", "carol", "bob"]);
    assert!(events.events.iter().all(|e| e.action == ShareAction::View));
}

#[tokio::test]
async fn long_user_agents_are_truncated_in_the_log() {
    let w = world().await;
    let (created, _) = w.share(&w.alice, "x", None, None).await;
    let long = "u".repeat(300);
    let resp = w
        .app
        .send(
            Req::new("POST", &format!("/v1/shares/{}/open", created.id))
                .token(&w.bob.token)
                .header("user-agent", &long),
        )
        .await;
    assert_eq!(resp.status, StatusCode::OK);
    let events: ShareEventsResponse = w.events(&w.alice, &created.id).await.json();
    assert_eq!(events.events[0].client.as_deref().map(str::len), Some(128));

    // no user agent at all is fine
    let resp = w
        .app
        .send(Req::new("POST", &format!("/v1/shares/{}/open", created.id)).token(&w.bob.token))
        .await;
    assert_eq!(resp.status, StatusCode::OK);
    let events: ShareEventsResponse = w.events(&w.alice, &created.id).await.json();
    assert_eq!(events.events[1].client, None);
}

#[tokio::test]
async fn shares_are_listed_newest_first() {
    let w = world().await;
    let (a, _) = w.share(&w.alice, "1", None, None).await;
    let (b, _) = w.share(&w.alice, "2", None, None).await;
    let (c, _) = w.share(&w.alice, "3", None, None).await;
    let list: ListSharesResponse = w.app.get("/v1/shares", Some(&w.alice.token)).await.json();
    let ids: Vec<&str> = list.shares.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, [c.id.as_str(), b.id.as_str(), a.id.as_str()]);
}

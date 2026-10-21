mod common;

use axum::http::StatusCode;
use common::*;
use dotvault_core::{
    api::{CreateItemRequest, ItemDetail, ListItemsResponse, UpdateItemRequest},
    crypto::{aad_item_name, aad_item_payload, open, seal},
};

const PW: &str = "correct horse battery staple";

fn update_request(
    vault_key: &dotvault_core::crypto::Key,
    id: &str,
    name: &str,
    payload: &str,
    version: u32,
) -> UpdateItemRequest {
    let (name_ct, payload_ct) = sealed_item(vault_key, id, name, payload, version);
    UpdateItemRequest {
        name_ct,
        payload_ct,
        version,
    }
}

#[tokio::test]
async fn create_list_get_update_delete() {
    let app = TestApp::new().await;
    let alice = app.register("alice", PW).await;
    let t = Some(alice.token.as_str());
    let id = new_item_id();

    // empty vault
    let list: ListItemsResponse = app.get("/v1/vault/items", t).await.json();
    assert!(list.items.is_empty());

    // create: version 1
    let req = create_item_request(
        &alice.vault_key,
        &id,
        "acme/api/prod",
        "DB_URL=postgres://one",
    );
    let resp = app.post("/v1/vault/items", t, &req).await;
    assert_eq!(resp.status, StatusCode::CREATED, "{}", resp.text());
    let created: ItemDetail = resp.json();
    assert_eq!(created.id, id);
    assert_eq!(created.version, 1);
    assert_eq!(created.payload_ct, req.payload_ct);

    // list: only ciphertext of the name, which the client can open
    let list: ListItemsResponse = app.get("/v1/vault/items", t).await.json();
    assert_eq!(list.items.len(), 1);
    assert_eq!(list.items[0].id, id);
    assert_eq!(list.items[0].version, 1);
    let name = open(
        &alice.vault_key,
        &list.items[0].name_ct,
        &aad_item_name(&id),
    )
    .unwrap();
    assert_eq!(&name[..], b"acme/api/prod");

    // get: payload decrypts under (id, version 1)
    let got: ItemDetail = app.get(&format!("/v1/vault/items/{id}"), t).await.json();
    assert_eq!(got.version, 1);
    let plain = open(&alice.vault_key, &got.payload_ct, &aad_item_payload(&id, 1)).unwrap();
    assert_eq!(&plain[..], b"DB_URL=postgres://one");
    assert!(open(&alice.vault_key, &got.payload_ct, &aad_item_payload(&id, 2)).is_err());

    // update to version 2, renaming it too
    let upd = update_request(
        &alice.vault_key,
        &id,
        "acme/api/production",
        "DB_URL=postgres://two",
        2,
    );
    let resp = app.put(&format!("/v1/vault/items/{id}"), t, &upd).await;
    assert_eq!(resp.status, StatusCode::OK, "{}", resp.text());
    let updated: ItemDetail = resp.json();
    assert_eq!(updated.version, 2);
    assert_eq!(updated.created_at, created.created_at);

    let got: ItemDetail = app.get(&format!("/v1/vault/items/{id}"), t).await.json();
    assert_eq!(got.version, 2);
    let plain = open(&alice.vault_key, &got.payload_ct, &aad_item_payload(&id, 2)).unwrap();
    assert_eq!(&plain[..], b"DB_URL=postgres://two");
    let name = open(&alice.vault_key, &got.name_ct, &aad_item_name(&id)).unwrap();
    assert_eq!(&name[..], b"acme/api/production");
    let list: ListItemsResponse = app.get("/v1/vault/items", t).await.json();
    assert_eq!(list.items[0].version, 2);

    // delete is soft: gone from the API...
    let resp = app.delete(&format!("/v1/vault/items/{id}"), t).await;
    assert_eq!(resp.status, StatusCode::NO_CONTENT);
    let list: ListItemsResponse = app.get("/v1/vault/items", t).await.json();
    assert!(list.items.is_empty());
    app.get(&format!("/v1/vault/items/{id}"), t)
        .await
        .assert_error(StatusCode::NOT_FOUND, "not_found");
    let upd3 = update_request(&alice.vault_key, &id, "x", "y", 3);
    app.put(&format!("/v1/vault/items/{id}"), t, &upd3)
        .await
        .assert_error(StatusCode::NOT_FOUND, "not_found");
    app.delete(&format!("/v1/vault/items/{id}"), t)
        .await
        .assert_error(StatusCode::NOT_FOUND, "not_found");

    // ...but still in the database, marked deleted
    let deleted_at: Option<i64> = sqlx::query_scalar("SELECT deleted_at FROM vault_items")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert!(deleted_at.is_some());
    // and a soft-deleted id cannot be reused
    app.post("/v1/vault/items", t, &req)
        .await
        .assert_error(StatusCode::CONFLICT, "conflict");
}

#[tokio::test]
async fn version_conflicts_are_rejected() {
    let app = TestApp::new().await;
    let alice = app.register("alice", PW).await;
    let t = Some(alice.token.as_str());
    let id = new_item_id();
    let path = format!("/v1/vault/items/{id}");
    app.post(
        "/v1/vault/items",
        t,
        &create_item_request(&alice.vault_key, &id, "n", "v1"),
    )
    .await;

    // must be exactly current + 1: replaying 1, repeating 2 and skipping to 4 all fail
    let attempts = [
        (1, "replay of current"),
        (0, "zero"),
        (3, "skips ahead"),
        (99, "far ahead"),
    ];
    for (version, why) in attempts {
        let upd = update_request(&alice.vault_key, &id, "n", "nope", version);
        app.put(&path, t, &upd)
            .await
            .assert_error(StatusCode::CONFLICT, "conflict");
        let _ = why;
    }
    let ok = update_request(&alice.vault_key, &id, "n", "v2", 2);
    assert_eq!(app.put(&path, t, &ok).await.status, StatusCode::OK);

    // a second writer holding a stale copy loses
    let stale = update_request(&alice.vault_key, &id, "n", "stale", 2);
    let resp = app.put(&path, t, &stale).await;
    resp.assert_error(StatusCode::CONFLICT, "conflict");
    assert!(
        resp.error().message.contains("version 2"),
        "{}",
        resp.error().message
    );

    // nothing was clobbered
    let got: ItemDetail = app.get(&path, t).await.json();
    assert_eq!(got.version, 2);
    let plain = open(&alice.vault_key, &got.payload_ct, &aad_item_payload(&id, 2)).unwrap();
    assert_eq!(&plain[..], b"v2");
    let versions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM vault_item_versions")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(versions, 2);

    // updating something that does not exist is 404, not 409
    let other = new_item_id();
    app.put(
        &format!("/v1/vault/items/{other}"),
        t,
        &update_request(&alice.vault_key, &other, "n", "x", 2),
    )
    .await
    .assert_error(StatusCode::NOT_FOUND, "not_found");
}

#[tokio::test]
async fn duplicate_ids_and_bad_ids() {
    let app = TestApp::new().await;
    let alice = app.register("alice", PW).await;
    let t = Some(alice.token.as_str());
    let id = new_item_id();
    let req = create_item_request(&alice.vault_key, &id, "n", "v");
    assert_eq!(
        app.post("/v1/vault/items", t, &req).await.status,
        StatusCode::CREATED
    );
    app.post("/v1/vault/items", t, &req)
        .await
        .assert_error(StatusCode::CONFLICT, "conflict");

    for bad in [
        "",
        "short",
        "AAAAAAAAAAAAAAAAAAAAAAA",
        "AAAAAAAAAAAAAAAAAAAA/+",
        "AAAAAAAAAAAAAAAAAAAAA=",
    ] {
        let req = CreateItemRequest {
            id: bad.to_string(),
            ..req.clone()
        };
        app.post("/v1/vault/items", t, &req)
            .await
            .assert_error(StatusCode::BAD_REQUEST, "bad_request");
    }
}

#[tokio::test]
async fn other_users_items_are_invisible() {
    let app = TestApp::new().await;
    let alice = app.register("alice", PW).await;
    let bob = app.register("bob", PW).await;
    let (ta, tb) = (Some(alice.token.as_str()), Some(bob.token.as_str()));
    let id = new_item_id();
    let path = format!("/v1/vault/items/{id}");
    app.post(
        "/v1/vault/items",
        ta,
        &create_item_request(&alice.vault_key, &id, "alice's", "ALICE_SECRET=1"),
    )
    .await;

    // Bob sees nothing, and every route says 404 (never 403)
    let list: ListItemsResponse = app.get("/v1/vault/items", tb).await.json();
    assert!(list.items.is_empty());
    let real_404 = app
        .get(&format!("/v1/vault/items/{}", new_item_id()), tb)
        .await;
    let cross = app.get(&path, tb).await;
    cross.assert_error(StatusCode::NOT_FOUND, "not_found");
    assert_eq!(
        cross.body, real_404.body,
        "indistinguishable from a missing item"
    );
    app.put(&path, tb, &update_request(&bob.vault_key, &id, "x", "y", 2))
        .await
        .assert_error(StatusCode::NOT_FOUND, "not_found");
    app.delete(&path, tb)
        .await
        .assert_error(StatusCode::NOT_FOUND, "not_found");

    // Alice's item survived Bob's attempts
    let got: ItemDetail = app.get(&path, ta).await.json();
    assert_eq!(got.version, 1);
    let plain = open(&alice.vault_key, &got.payload_ct, &aad_item_payload(&id, 1)).unwrap();
    assert_eq!(&plain[..], b"ALICE_SECRET=1");

    // Bob can use the same id for his own item without a clash or a leak
    let resp = app
        .post(
            "/v1/vault/items",
            tb,
            &create_item_request(&bob.vault_key, &id, "bob's", "BOB_SECRET=1"),
        )
        .await;
    assert_eq!(resp.status, StatusCode::CREATED);
    let bobs: ItemDetail = app.get(&path, tb).await.json();
    assert!(open(&bob.vault_key, &bobs.payload_ct, &aad_item_payload(&id, 1)).is_ok());
    let alices: ItemDetail = app.get(&path, ta).await.json();
    assert_eq!(alices.payload_ct, got.payload_ct);
}

#[tokio::test]
async fn only_the_newest_twenty_versions_are_kept() {
    let app = TestApp::new().await;
    let alice = app.register("alice", PW).await;
    let t = Some(alice.token.as_str());
    let id = new_item_id();
    let path = format!("/v1/vault/items/{id}");
    app.post(
        "/v1/vault/items",
        t,
        &create_item_request(&alice.vault_key, &id, "n", "payload v1"),
    )
    .await;
    for v in 2..=25u32 {
        let upd = update_request(&alice.vault_key, &id, "n", &format!("payload v{v}"), v);
        let resp = app.put(&path, t, &upd).await;
        assert_eq!(resp.status, StatusCode::OK, "v{v}: {}", resp.text());
    }

    let (count, min, max): (i64, i64, i64) =
        sqlx::query_as("SELECT COUNT(*), MIN(version), MAX(version) FROM vault_item_versions")
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!((count, min, max), (20, 6, 25));

    // the current version is always readable
    let got: ItemDetail = app.get(&path, t).await.json();
    assert_eq!(got.version, 25);
    let plain = open(
        &alice.vault_key,
        &got.payload_ct,
        &aad_item_payload(&id, 25),
    )
    .unwrap();
    assert_eq!(&plain[..], b"payload v25");
}

#[tokio::test]
async fn size_limits() {
    let app = TestApp::new().await;
    let alice = app.register("alice", PW).await;
    let t = Some(alice.token.as_str());
    let mk = |name_len: usize, payload_len: usize| CreateItemRequest {
        id: new_item_id(),
        name_ct: vec![1; name_len],
        payload_ct: vec![2; payload_len],
    };

    // exactly at the limits is fine
    let r = app
        .post("/v1/vault/items", t, &mk(4 * 1024, 256 * 1024))
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());

    // one byte over is 413 payload_too_large
    app.post("/v1/vault/items", t, &mk(4 * 1024 + 1, 10))
        .await
        .assert_error(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large");
    app.post("/v1/vault/items", t, &mk(10, 256 * 1024 + 1))
        .await
        .assert_error(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large");

    // empty is a plain 400
    app.post("/v1/vault/items", t, &mk(0, 10))
        .await
        .assert_error(StatusCode::BAD_REQUEST, "bad_request");
    app.post("/v1/vault/items", t, &mk(10, 0))
        .await
        .assert_error(StatusCode::BAD_REQUEST, "bad_request");

    // updates are held to the same limits
    let id = new_item_id();
    app.post(
        "/v1/vault/items",
        t,
        &create_item_request(&alice.vault_key, &id, "n", "v"),
    )
    .await;
    let too_big = UpdateItemRequest {
        name_ct: seal(&alice.vault_key, b"n", b"").unwrap(),
        payload_ct: vec![0; 256 * 1024 + 1],
        version: 2,
    };
    app.put(&format!("/v1/vault/items/{id}"), t, &too_big)
        .await
        .assert_error(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large");
}

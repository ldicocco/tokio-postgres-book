mod common;

use axum::body::Body;
use axum::http::{Method, StatusCode};
use common::TestDb;
use serde_json::json;

#[tokio::test]
async fn create_read_and_the_errors_a_client_can_cause() {
    let db = TestDb::new().await;
    let app = db.app(4);

    let created = app
        .post(
            "/products",
            json!({ "sku": "CHAIR-01", "name": "Chair", "priceCents": 4999 }),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED);
    assert_eq!(created.headers["etag"], "\"1\"");
    assert_eq!(created.body["version"], 1);

    let read = app.get("/products/CHAIR-01").await;
    assert_eq!(read.status, StatusCode::OK);
    assert_eq!(read.body["priceCents"], 4999);

    let duplicate = app
        .post(
            "/products",
            json!({ "sku": "CHAIR-01", "name": "Again", "priceCents": 1 }),
        )
        .await;
    assert_eq!(duplicate.status, StatusCode::CONFLICT);
    assert_eq!(
        duplicate.body,
        json!({ "error": "conflict", "field": "sku", "message": "a product with this SKU already exists" })
    );

    let free = app
        .post(
            "/products",
            json!({ "sku": "LAMP-01", "name": "Lamp", "priceCents": 0 }),
        )
        .await;
    assert_eq!(free.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(free.body["field"], "priceCents");

    let malformed = app
        .post(
            "/products",
            json!({ "sku": "lamp", "name": "Lamp", "priceCents": 10 }),
        )
        .await;
    assert_eq!(malformed.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(malformed.body["error"], "invalid");

    assert_eq!(
        app.get("/products/NOPE-01").await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn updates_check_the_version_the_client_saw() {
    let db = TestDb::new().await;
    let app = db.app(4);
    app.product("CHAIR-01", 4999).await;

    let patch = |version: &'static str, price: i64| {
        let app = app.clone();
        async move {
            app.json(
                Method::PATCH,
                "/products/CHAIR-01",
                &[("if-match", version)],
                json!({ "priceCents": price }),
            )
            .await
        }
    };
    let first = patch("\"1\"", 3999).await;
    assert_eq!(first.status, StatusCode::OK);
    assert_eq!(first.headers["etag"], "\"2\"");

    // A second client still holding version 1 loses, and learns the current version.
    let second = patch("\"1\"", 2999).await;
    assert_eq!(second.status, StatusCode::PRECONDITION_FAILED);
    assert_eq!(second.body["currentVersion"], 2);
    assert_eq!(app.get("/products/CHAIR-01").await.body["priceCents"], 3999);
}

#[tokio::test]
async fn keyset_pagination_walks_the_catalog() {
    let db = TestDb::new().await;
    let app = db.app(4);
    for i in 1..=7 {
        app.product(&format!("ITEM-0{i}"), 100).await;
    }
    let mut seen = Vec::new();
    let mut uri = "/products?limit=3".to_owned();
    loop {
        let page = app.get(&uri).await;
        assert_eq!(page.status, StatusCode::OK);
        for item in page.body["items"].as_array().unwrap() {
            seen.push(item["sku"].as_str().unwrap().to_owned());
        }
        match page.body["nextCursor"].as_str() {
            Some(cursor) => uri = format!("/products?limit=3&cursor={cursor}"),
            None => break,
        }
    }
    let expected: Vec<String> = (1..=7).map(|i| format!("ITEM-0{i}")).collect();
    assert_eq!(seen, expected);
    assert_eq!(
        app.get("/products?cursor=zz").await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn a_batch_keeps_the_good_items_thanks_to_savepoints() {
    let db = TestDb::new().await;
    let app = db.app(4);
    app.product("CHAIR-01", 4999).await;

    let reply = app
        .post(
            "/products/batch",
            json!([
                { "sku": "LAMP-01", "name": "Lamp", "priceCents": 2999 },
                { "sku": "CHAIR-01", "name": "Duplicate", "priceCents": 1 },
                { "sku": "RUG-01", "name": "Rug", "priceCents": -5 },
                { "sku": "VASE-01", "name": "Vase", "priceCents": 1999 }
            ]),
        )
        .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
    let statuses: Vec<&str> = reply
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["status"].as_str().unwrap())
        .collect();
    assert_eq!(statuses, ["created", "rejected", "rejected", "created"]);
    assert_eq!(reply.body[1]["error"]["error"], "conflict");
    assert_eq!(reply.body[2]["error"]["field"], "priceCents");
    // The items after a failure were inserted in the same, still healthy, transaction.
    assert_eq!(app.get("/products/VASE-01").await.status, StatusCode::OK);
    assert_eq!(
        app.get("/products/RUG-01").await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn csv_import_merges_and_export_reads_it_back() {
    let db = TestDb::new().await;
    let app = db.app(4);
    let import = |csv: &'static str| {
        let app = app.clone();
        async move {
            app.send(Method::POST, "/products/import", &[], Body::from(csv))
                .await
        }
    };

    let first =
        import("sku,name,price_cents,tags\nCHAIR-01,Chair,4999,garden|sale\nLAMP-01,Lamp,2999,\n")
            .await;
    assert_eq!(
        first.body,
        json!({ "rows": 2, "inserted": 2, "updated": 0, "unchanged": 0 }),
        "{}",
        first.text
    );

    let second = import("sku,name,price_cents,tags\nCHAIR-01,Chair,3999,garden|sale\nLAMP-01,Lamp,2999,\nVASE-01,Vase,999,\n").await;
    assert_eq!(
        second.body,
        json!({ "rows": 3, "inserted": 1, "updated": 1, "unchanged": 1 })
    );

    let chair = app.get("/products/CHAIR-01").await;
    assert_eq!(chair.body["tags"], json!(["garden", "sale"]));
    assert_eq!(chair.body["version"], 2);

    let export = app.get("/products/export.csv").await;
    assert_eq!(export.headers["content-type"], "text/csv");
    assert_eq!(
        export.text,
        "sku,name,price_cents,tags\nCHAIR-01,Chair,3999,garden|sale\nLAMP-01,Lamp,2999,\nVASE-01,Vase,999,\n"
    );
}

#[tokio::test]
async fn a_bad_csv_changes_nothing_and_says_where_it_is_bad() {
    let db = TestDb::new().await;
    let app = db.app(4);
    let send = |csv: &'static str| {
        let app = app.clone();
        async move {
            app.send(Method::POST, "/products/import", &[], Body::from(csv))
                .await
        }
    };

    let not_a_number =
        send("sku,name,price_cents,tags\nCHAIR-01,Chair,4999,\nLAMP-01,Lamp,cheap,\n").await;
    assert_eq!(not_a_number.status, StatusCode::UNPROCESSABLE_ENTITY);
    let message = not_a_number.body["message"].as_str().unwrap();
    assert!(message.contains("line 3"), "{message}");

    let wrong_header = send("sku,title,price_cents,tags\n").await;
    assert_eq!(wrong_header.status, StatusCode::UNPROCESSABLE_ENTITY);

    let twice = send("sku,name,price_cents,tags\nCHAIR-01,Chair,1,\nCHAIR-01,Chair,2,\n").await;
    assert_eq!(twice.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(twice.body["field"], "sku");

    let bad_sku = send("sku,name,price_cents,tags\nchair,Chair,1,\n").await;
    assert_eq!(bad_sku.status, StatusCode::UNPROCESSABLE_ENTITY);

    // Every import ran in a transaction that rolled back: the first good row is not there.
    assert_eq!(
        app.get("/products/CHAIR-01").await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn the_ndjson_stream_has_one_line_per_product() {
    let db = TestDb::new().await;
    let app = db.app(2);
    for sku in ["A-01", "B-01", "C-01"] {
        app.product(&format!("SKU{sku}"), 100).await;
    }
    let reply = app.get("/products/stream").await;
    assert_eq!(reply.headers["content-type"], "application/x-ndjson");
    let skus: Vec<String> = reply
        .text
        .lines()
        .map(|line| {
            serde_json::from_str::<serde_json::Value>(line).unwrap()["sku"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(skus, ["SKUA-01", "SKUB-01", "SKUC-01"]);
    // The connection went back to the pool when the body was done.
    assert_eq!(
        app.state.pool.status().available,
        app.state.pool.status().size
    );
}

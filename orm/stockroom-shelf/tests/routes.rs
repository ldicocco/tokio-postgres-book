//! The ported routes answer as stockroom's do: each scenario runs against both routers, and
//! every reply is compared (`common::both`). The scenarios are stockroom's own tests for those
//! routes (`stockroom/tests/products.rs`, `orders.rs`), plus what a port could get wrong.

mod common;

use axum::body::Body;
use axum::http::{Method, StatusCode};
use common::{Reply, TestApp, TestDb, both, shelf_app};
use serde_json::{Value, json};

fn address() -> Value {
    json!({ "street": "Main 1", "city": "Berlin", "postcode": "10115", "country": "DE" })
}

async fn patch(app: &TestApp, sku: &str, if_match: Option<&str>, body: Value) -> Reply {
    let headers: Vec<(&str, &str)> = if_match.map(|v| ("if-match", v)).into_iter().collect();
    app.json(Method::PATCH, &format!("/products/{sku}"), &headers, body)
        .await
}

#[tokio::test]
async fn stores_are_created_and_listed() {
    both(async |app: &TestApp| {
        let mut replies = Vec::new();
        for code in ["spandau", "mitte"] {
            let store = json!({ "code": code, "name": code, "address": address() });
            let created = app.post("/stores", store).await;
            assert_eq!(created.status, StatusCode::CREATED, "{}", created.text);
            replies.push(created);
        }
        let again = json!({ "code": "mitte", "name": "Again", "address": address() });
        let duplicate = app.post("/stores", again).await;
        assert_eq!(duplicate.status, StatusCode::CONFLICT);
        assert_eq!(duplicate.body["field"], "code");
        replies.push(duplicate);
        let shouting = json!({ "code": "MITTE", "name": "Mitte", "address": address() });
        let bad_code = app.post("/stores", shouting).await;
        assert_eq!(bad_code.status, StatusCode::UNPROCESSABLE_ENTITY);
        replies.push(bad_code);
        let no_address = app
            .post("/stores", json!({ "code": "wedding", "name": "W" }))
            .await;
        assert_eq!(no_address.status, StatusCode::UNPROCESSABLE_ENTITY);
        replies.push(no_address);
        let list = app.get("/stores").await;
        let codes: Vec<&str> = list
            .body
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["code"].as_str().unwrap())
            .collect();
        assert_eq!(codes, ["mitte", "spandau"]);
        replies.push(list);
        replies
    })
    .await;
}

#[tokio::test]
async fn create_read_and_the_errors_a_client_can_cause() {
    both(async |app: &TestApp| {
        let mut replies = Vec::new();
        let chair = json!({ "sku": "CHAIR-01", "name": "Chair", "priceCents": 4999,
                            "tags": ["garden"], "attributes": { "colour": "green" } });
        let created = app.post("/products", chair).await;
        assert_eq!(created.status, StatusCode::CREATED);
        assert_eq!(created.headers["etag"], "\"1\"");
        replies.push(created);
        let read = app.get("/products/CHAIR-01").await;
        assert_eq!(read.body["attributes"]["colour"], "green");
        replies.push(read);
        for body in [
            json!({ "sku": "CHAIR-01", "name": "Again", "priceCents": 1 }),
            json!({ "sku": "LAMP-01", "name": "Lamp", "priceCents": 0 }),
            json!({ "sku": "lamp", "name": "Lamp", "priceCents": 10 }),
            json!({ "sku": "LAMP-01", "name": "", "priceCents": 10 }),
        ] {
            let refused = app.post("/products", body).await;
            assert!(refused.status.is_client_error(), "{}", refused.text);
            replies.push(refused);
        }
        for uri in ["/products/NOPE-01", "/products/nope"] {
            replies.push(app.get(uri).await);
        }
        replies
    })
    .await;
}

#[tokio::test]
async fn updates_check_the_version_the_client_saw() {
    both(async |app: &TestApp| {
        app.product("CHAIR-01", 4999).await;
        let mut replies = Vec::new();
        let first = patch(
            app,
            "CHAIR-01",
            Some("\"1\""),
            json!({ "priceCents": 3999 }),
        )
        .await;
        assert_eq!(first.status, StatusCode::OK);
        assert_eq!(first.headers["etag"], "\"2\"");
        replies.push(first);
        // A second client still holding version 1 loses, and learns the current version.
        let second = patch(
            app,
            "CHAIR-01",
            Some("\"1\""),
            json!({ "priceCents": 2999 }),
        )
        .await;
        assert_eq!(second.status, StatusCode::PRECONDITION_FAILED);
        assert_eq!(second.body["currentVersion"], 2);
        replies.push(second);
        // Without If-Match, whatever the version is.
        let any = patch(
            app,
            "CHAIR-01",
            None,
            json!({ "tags": ["sale"], "attributes": { "a": 1 } }),
        )
        .await;
        assert_eq!(any.status, StatusCode::OK);
        assert_eq!(any.body["version"], 3);
        assert_eq!(
            any.body["priceCents"], 3999,
            "an absent field keeps its value"
        );
        replies.push(any);
        replies.push(patch(app, "CHAIR-01", Some("*"), json!({ "name": "Chair" })).await);
        for (sku, if_match, body) in [
            ("CHAIR-01", None, json!({ "version": 9 })),
            ("CHAIR-01", Some("soon"), json!({ "name": "x" })),
            ("CHAIR-01", None, json!({ "priceCents": -1 })),
            ("NOPE-01", None, json!({ "name": "x" })),
            ("NOPE-01", Some("\"1\""), json!({ "name": "x" })),
        ] {
            let refused = patch(app, sku, if_match, body).await;
            assert!(refused.status.is_client_error(), "{}", refused.text);
            replies.push(refused);
        }
        replies.push(app.get("/products/CHAIR-01").await);
        replies
    })
    .await;
}

#[tokio::test]
async fn keyset_pagination_walks_the_catalog_with_and_without_a_tag() {
    both(async |app: &TestApp| {
        for i in 1..=7 {
            let tags = if i % 2 == 0 { json!(["sale"]) } else { json!([]) };
            let item = json!({ "sku": format!("ITEM-0{i}"), "name": "Item", "priceCents": 100, "tags": tags });
            assert_eq!(app.post("/products", item).await.status, StatusCode::CREATED);
        }
        let mut replies = Vec::new();
        for (first, expected) in [
            ("/products?limit=3", vec![1, 2, 3, 4, 5, 6, 7]),
            ("/products?limit=2&tag=sale", vec![2, 4, 6]),
        ] {
            let mut seen = Vec::new();
            let mut uri = first.to_owned();
            loop {
                let page = app.get(&uri).await;
                assert_eq!(page.status, StatusCode::OK, "{}", page.text);
                for item in page.body["items"].as_array().unwrap() {
                    seen.push(item["sku"].as_str().unwrap().to_owned());
                }
                let next = page.body["nextCursor"].as_str().map(str::to_owned);
                replies.push(page);
                match next {
                    Some(cursor) => uri = format!("{first}&cursor={cursor}"),
                    None => break,
                }
            }
            let expected: Vec<String> = expected.iter().map(|i| format!("ITEM-0{i}")).collect();
            assert_eq!(seen, expected, "{first}");
        }
        for uri in ["/products?cursor=zz", "/products?limit=x", "/products?tag=none"] {
            replies.push(app.get(uri).await);
        }
        replies
    })
    .await;
}

#[tokio::test]
async fn a_batch_keeps_the_good_items_thanks_to_savepoints() {
    both(async |app: &TestApp| {
        app.product("CHAIR-01", 4999).await;
        let batch = json!([
            { "sku": "LAMP-01", "name": "Lamp", "priceCents": 2999 },
            { "sku": "CHAIR-01", "name": "Duplicate", "priceCents": 1 },
            { "sku": "RUG-01", "name": "Rug", "priceCents": -5 },
            { "sku": "VASE-01", "name": "Vase", "priceCents": 1999 }
        ]);
        let reply = app.post("/products/batch", batch).await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
        let statuses: Vec<&str> = reply
            .body
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["status"].as_str().unwrap())
            .collect();
        assert_eq!(statuses, ["created", "rejected", "rejected", "created"]);
        let empty = app.post("/products/batch", json!([])).await;
        assert_eq!(empty.status, StatusCode::UNPROCESSABLE_ENTITY);
        vec![
            reply,
            empty,
            app.get("/products/VASE-01").await,
            app.get("/products/RUG-01").await,
        ]
    })
    .await;
}

#[tokio::test]
async fn the_ndjson_stream_has_one_line_per_product() {
    both(async |app: &TestApp| {
        for sku in ["SKUC-01", "SKUA-01", "SKUB-01"] {
            app.product(sku, 100).await;
        }
        let reply = app.get("/products/stream").await;
        assert_eq!(reply.headers["content-type"], "application/x-ndjson");
        let skus: Vec<String> = common::masked(&reply)
            .as_array()
            .unwrap()
            .iter()
            .map(|line| line["sku"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(skus, ["SKUA-01", "SKUB-01", "SKUC-01"]);
        // The connection went back to the pool when the body was done.
        let status = app.state.pool.status();
        assert_eq!(status.available, status.size);
        vec![reply]
    })
    .await;
}

async fn shop(app: &TestApp) {
    app.store("mitte").await;
    app.product("CHAIR-01", 4999).await;
    app.product("LAMP-01", 2999).await;
    app.stock("mitte", "CHAIR-01", 5).await;
    app.stock("mitte", "LAMP-01", 2).await;
}

#[tokio::test]
async fn placing_an_order_prices_it_and_reserves_its_stock() {
    both(async |app: &TestApp| {
        shop(app).await;
        // The same SKU twice is one line of 3.
        let placed = app
            .order("mitte", &[("LAMP-01", 1), ("CHAIR-01", 2), ("CHAIR-01", 1)])
            .await;
        assert_eq!(placed.status, StatusCode::CREATED, "{}", placed.text);
        assert_eq!(placed.body["totalCents"], 3 * 4999 + 2999);
        assert_eq!(
            placed.body["lines"],
            json!([
                { "sku": "CHAIR-01", "quantity": 3, "unitPriceCents": 4999 },
                { "sku": "LAMP-01", "quantity": 1, "unitPriceCents": 2999 }
            ])
        );
        assert_eq!(app.level("mitte", "CHAIR-01").await, (5, 3, 2));
        let id = placed.body["id"].as_str().unwrap().to_owned();
        let read = app.get(&format!("/orders/{id}")).await;
        assert_eq!(
            read.body["customer"],
            json!({ "name": "Ada", "email": "ada@example.com" })
        );
        assert_eq!(read.body, placed.body, "read back as placed");
        vec![placed, read]
    })
    .await;
}

#[tokio::test]
async fn an_order_that_cannot_be_placed_reserves_nothing() {
    both(async |app: &TestApp| {
        shop(app).await;
        let mut replies = Vec::new();
        let refused = app.order("mitte", &[("CHAIR-01", 1), ("LAMP-01", 3)]).await;
        assert_eq!(refused.status, StatusCode::CONFLICT);
        assert_eq!(refused.body["available"], 2);
        replies.push(refused);
        // The chair's reservation was rolled back with the rest.
        assert_eq!(app.level("mitte", "CHAIR-01").await, (5, 0, 5));
        app.product("VASE-01", 999).await; // in the catalog, never stocked here
        for (store, lines) in [
            ("mitte", vec![("VASE-01", 1)]),
            ("mitte", vec![("NOPE-01", 1)]),
            ("nowhere", vec![("CHAIR-01", 1)]),
            ("nowhere", vec![("CHAIR-01", 600), ("CHAIR-01", 600)]),
            ("mitte", vec![("CHAIR-01", 0)]),
            ("mitte", vec![]),
        ] {
            let reply = app.order(store, &lines).await;
            assert!(reply.status.is_client_error(), "{}", reply.text);
            replies.push(reply);
        }
        let no_customer = app
            .post("/orders", json!({ "store": "mitte", "lines": [] }))
            .await;
        replies.push(no_customer);
        let bad_customer = app
            .post(
                "/orders",
                json!({ "store": "mitte", "customer": { "name": "Ada", "email": "ada" },
                                     "lines": [{ "sku": "CHAIR-01", "quantity": 1 }] }),
            )
            .await;
        replies.push(bad_customer);
        assert_eq!(app.level("mitte", "LAMP-01").await, (2, 0, 2));
        replies
    })
    .await;
}

#[tokio::test]
async fn an_order_placed_on_shelf_moves_through_stockroom_s_transitions() {
    both(async |app: &TestApp| {
        shop(app).await;
        let placed = app.order("mitte", &[("CHAIR-01", 2)]).await;
        let id = placed.body["id"].as_str().unwrap().to_owned();
        let uri = format!("/orders/{id}/transition");
        let mut replies = vec![placed];
        for (to, if_match, expected) in [
            ("ready", None, StatusCode::CONFLICT),
            ("paid", Some("\"1\""), StatusCode::OK),
            ("ready", Some("\"1\""), StatusCode::PRECONDITION_FAILED),
            ("ready", Some("\"2\""), StatusCode::OK),
            ("collected", None, StatusCode::OK),
        ] {
            let headers: Vec<(&str, &str)> =
                if_match.map(|v| ("if-match", v)).into_iter().collect();
            let moved = app
                .json(Method::POST, &uri, &headers, json!({ "to": to }))
                .await;
            assert_eq!(moved.status, expected, "{to}: {}", moved.text);
            replies.push(moved);
        }
        assert_eq!(app.level("mitte", "CHAIR-01").await, (3, 0, 3));
        replies.push(app.get(&format!("/orders/{id}")).await);
        replies
    })
    .await;
}

#[tokio::test]
async fn unknown_orders_are_not_found() {
    both(async |app: &TestApp| {
        vec![
            app.get("/orders/01a1b1ee-0000-7000-8000-000000000000")
                .await,
            app.get("/orders/not-a-uuid").await,
        ]
    })
    .await;
}

#[tokio::test]
async fn the_routes_that_are_not_ported_are_stockroom_s() {
    both(async |app: &TestApp| {
        let import = app
            .send(
                Method::POST,
                "/products/import",
                &[],
                Body::from("sku,name,price_cents,tags\nCHAIR-01,Chair,4999,sale\n"),
            )
            .await;
        assert_eq!(import.status, StatusCode::OK, "{}", import.text);
        let export = app.get("/products/export.csv").await;
        assert_eq!(
            export.text,
            "sku,name,price_cents,tags\nCHAIR-01,Chair,4999,sale\n"
        );
        vec![
            import,
            export,
            app.get("/products/CHAIR-01").await,
            app.get("/healthz").await,
            app.get("/stores/nowhere/stock").await,
            app.get("/nope").await,
        ]
    })
    .await;
}

/// The replies alone cannot tell which router's handler answered, since they are meant to be
/// the same. The statements can: on a pool of one connection, each ported route leaves shelf's
/// statement in deadpool's cache, and an unported one leaves stockroom's.
#[tokio::test]
async fn the_ported_routes_send_shelf_s_statements() {
    use shelf::{Insert, Table};
    use stockroom::domain::Sku;
    use stockroom_shelf::repo::orders::OrderRow;
    use stockroom_shelf::repo::products::{NewProduct, Product};
    use stockroom_shelf::repo::stores::{NewStore, Store};

    let db = TestDb::new().await;
    let app = shelf_app(&db, 1);
    app.store("mitte").await;
    app.product("CHAIR-01", 4999).await;
    app.stock("mitte", "CHAIR-01", 5).await;
    let placed = app.order("mitte", &[("CHAIR-01", 1)]).await;
    let id = placed.body["id"].as_str().unwrap();
    assert_eq!(
        app.get(&format!("/orders/{id}")).await.status,
        StatusCode::OK
    );
    assert_eq!(app.get("/products/CHAIR-01").await.status, StatusCode::OK);
    assert_eq!(app.get("/stores").await.status, StatusCode::OK);

    let sku = Sku::parse("CHAIR-01").unwrap();
    let by_sku = Product::query().filter(Product::SKU.eq(&sku));
    let shelf_statements = [
        NewStore::INSERT.to_owned(),
        NewProduct::INSERT.to_owned(),
        OrderRow::FIND.to_owned(),
        format!("{} LIMIT 1", by_sku.sql()),
        Store::query().order_by(Store::CODE.asc()).sql(),
    ];
    let connection = app.state.pool.get().await.unwrap();
    for sql in &shelf_statements {
        let cached = connection.statement_cache.remove(sql, &[]);
        assert!(cached.is_some(), "not sent: {sql}");
    }
    // `POST /stores/mitte/stock/adjust` is stockroom's, and so is its SQL.
    let stockroom_sql = "SELECT id FROM stores WHERE code = $1";
    assert!(
        connection
            .statement_cache
            .remove(stockroom_sql, &[])
            .is_some()
    );
}

/// A client that hangs up mid-stream leaves no busy connection in the pool: shelf's stream runs
/// inside a body that owns its connection through stockroom's `CancelOnDrop`, which cancels the
/// query and discards the connection (stockroom's test, on this crate's router alone).
#[tokio::test]
async fn a_client_that_hangs_up_mid_stream_leaves_no_busy_connection() {
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    let db = TestDb::new().await;
    let app = shelf_app(&db, 2);
    db.client()
        .await
        .batch_execute(
            "INSERT INTO products (sku, name, price_cents)
             SELECT 'SKU' || lpad(i::text, 6, '0'), 'Product ' || i, 100
             FROM generate_series(1, 50000) i",
        )
        .await
        .unwrap();

    let request = axum::http::Request::get("/products/stream")
        .body(Body::empty())
        .unwrap();
    let response = app.router.clone().oneshot(request).await.unwrap();
    let mut body = response.into_body();
    body.frame().await.unwrap().unwrap(); // the first rows arrive...
    drop(body); // ...and the client hangs up

    let pool = &app.state.pool;
    let mut waited = 0;
    while pool.status().size > 0 && waited < 100 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        waited += 1;
    }
    assert_eq!(pool.status().size, 0, "{:?}", pool.status());
    assert_eq!(app.get("/products?limit=1").await.status, StatusCode::OK);
}

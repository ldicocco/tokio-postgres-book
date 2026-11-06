//! `stockroom seed`: a realistic amount of data, loaded with binary COPY (Chapter 10).

use std::collections::BTreeMap;
use std::pin::pin;

use anyhow::bail;
use clap::Args;
use jiff::{Timestamp, ToSpan};
use serde_json::json;
use tokio_postgres::Client;
use tokio_postgres::binary_copy::BinaryCopyInWriter;
use tokio_postgres::types::Type;
use uuid::Uuid;

use crate::domain::{Address, Cents, Sku};

#[derive(Debug, Clone, Args)]
pub struct SeedArgs {
    #[arg(long, default_value_t = 3)]
    pub stores: usize,
    #[arg(long, default_value_t = 1_000)]
    pub products: usize,
    /// Stock movements (deliveries) spread over the last 90 days; they make up on-hand stock.
    #[arg(long, default_value_t = 20_000)]
    pub movements: usize,
    /// Empty every table first.
    #[arg(long)]
    pub reset: bool,
    /// Seed for the random numbers: the same seed gives the same data.
    #[arg(long, default_value_t = 42)]
    pub seed: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeedReport {
    pub stores: u64,
    pub products: u64,
    pub stock_rows: u64,
    pub movements: u64,
}

const TAGS: [&str; 6] = ["garden", "tools", "paint", "lighting", "kitchen", "outdoor"];

pub async fn run(client: &mut Client, args: &SeedArgs) -> anyhow::Result<SeedReport> {
    if args.stores == 0 || args.products == 0 || args.products > 99_999 {
        bail!("seed at least one store and 1–99999 products");
    }
    let tx = client.transaction().await?;
    if args.reset {
        tx.batch_execute(
            "TRUNCATE stores, products, stock, orders, order_lines, stock_movements, outbox RESTART IDENTITY",
        )
        .await?;
    } else if tx
        .query_one("SELECT EXISTS (SELECT FROM products)", &[])
        .await?
        .get(0)
    {
        bail!("the catalog is not empty: pass --reset to replace it");
    }
    let mut rng = fastrand::Rng::with_seed(args.seed);

    // Stores: a few rows, as three arrays (one of them of a composite type) zipped by
    // `unnest`. In the select list, not in FROM: in FROM, `unnest` of an `address[]` would
    // expand each address into its four fields.
    let codes: Vec<String> = (1..=args.stores).map(|i| format!("store-{i:02}")).collect();
    let names: Vec<String> = (1..=args.stores).map(|i| format!("Store {i}")).collect();
    let addresses: Vec<Address> = (1..=args.stores)
        .map(|i| Address {
            street: format!("Market Street {i}"),
            city: "Berlin".into(),
            postcode: format!("{:05}", 10_115 + i),
            country: "DE".into(),
        })
        .collect();
    let store_ids: Vec<Uuid> = tx
        .query(
            "INSERT INTO stores (code, name, address)
             SELECT unnest($1::text[]), unnest($2::text[]), unnest($3::address[])
             RETURNING id",
            &[&codes, &names, &addresses],
        )
        .await?
        .iter()
        .map(|row| row.get(0))
        .collect();

    // Products: binary COPY. Every value is sent in its binary wire format, so the column
    // types are named up front, one per column, in COPY's column order. The `sku` column is
    // a domain, and in COPY a domain travels as its base type: `TEXT`.
    let sink = tx
        .copy_in(
            "COPY products (sku, name, price_cents, tags, attributes) FROM STDIN (FORMAT binary)",
        )
        .await?;
    let writer = BinaryCopyInWriter::new(
        sink,
        &[
            Type::TEXT,
            Type::TEXT,
            Type::INT8,
            Type::TEXT_ARRAY,
            Type::JSONB,
        ],
    );
    let mut writer = pin!(writer);
    for i in 1..=args.products {
        let sku = Sku::parse(&format!("SKU{i:05}"))?;
        let name = format!("Product {i}");
        let price = Cents(rng.i64(99..=99_999));
        let tags: Vec<&str> = (0..rng.usize(0..=2))
            .map(|_| TAGS[rng.usize(..TAGS.len())])
            .collect();
        let attributes = json!({ "weightGrams": rng.u32(50..=20_000) });
        writer
            .as_mut()
            .write(&[&sku, &name, &price, &tags, &attributes])
            .await?;
    }
    let products = writer.as_mut().finish().await?;

    let product_ids: Vec<Uuid> = tx
        .query("SELECT id FROM products ORDER BY sku", &[])
        .await?
        .iter()
        .map(|row| row.get(0))
        .collect();

    // Movements first, in memory, so that on-hand stock is exactly what they add up to.
    let now = Timestamp::now();
    let mut on_hand: BTreeMap<(Uuid, Uuid), i32> = BTreeMap::new();
    let mut movements = Vec::with_capacity(args.movements);
    for _ in 0..args.movements {
        let key = (
            store_ids[rng.usize(..store_ids.len())],
            product_ids[rng.usize(..product_ids.len())],
        );
        let delta = rng.i32(1..=24);
        *on_hand.entry(key).or_default() += delta;
        let at = now.checked_sub(rng.i64(0..90 * 24 * 3600).seconds())?;
        movements.push((key, delta, at));
    }

    let sink = tx
        .copy_in("COPY stock (store_id, product_id, on_hand) FROM STDIN (FORMAT binary)")
        .await?;
    let writer = BinaryCopyInWriter::new(sink, &[Type::UUID, Type::UUID, Type::INT4]);
    let mut writer = pin!(writer);
    for ((store, product), units) in &on_hand {
        writer.as_mut().write(&[store, product, units]).await?;
    }
    let stock_rows = writer.as_mut().finish().await?;

    let sink = tx
        .copy_in("COPY stock_movements (store_id, product_id, delta, reason, at) FROM STDIN (FORMAT binary)")
        .await?;
    let writer = BinaryCopyInWriter::new(
        sink,
        &[
            Type::UUID,
            Type::UUID,
            Type::INT4,
            Type::TEXT,
            Type::TIMESTAMPTZ,
        ],
    );
    let mut writer = pin!(writer);
    for ((store, product), delta, at) in &movements {
        writer
            .as_mut()
            .write(&[store, product, delta, &"delivery", at])
            .await?;
    }
    let movements = writer.as_mut().finish().await?;

    tx.commit().await?;
    Ok(SeedReport {
        stores: store_ids.len() as u64,
        products,
        stock_rows,
        movements,
    })
}

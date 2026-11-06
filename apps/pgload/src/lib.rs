//! pgload: five ways to load the same rows, from slowest to fastest (Chapter 10).
//!
//! | Strategy | Statements | Round trips for N rows |
//! |---|---|---|
//! | [`Strategy::Insert`] | one prepared `INSERT` per row, each awaited | N |
//! | [`Strategy::Pipelined`] | the same, 64 in flight at a time | about N / 64 |
//! | [`Strategy::Unnest`] | one `INSERT … SELECT FROM unnest(arrays)` per 1,000 rows | N / 1,000 |
//! | [`Strategy::CopyText`] | `COPY … FROM STDIN (FORMAT csv)` | a stream, no waiting |
//! | [`Strategy::CopyBinary`] | `COPY … FROM STDIN (FORMAT binary)` | a stream, no waiting |
//!
//! Each run loads into a fresh temporary table, in one transaction, and checks the count.

use std::pin::pin;

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt, TryStreamExt, stream};
use jiff::Timestamp;
use tokio_postgres::binary_copy::BinaryCopyInWriter;
use tokio_postgres::types::Type;
use tokio_postgres::{Client, Error, Transaction};

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub id: i64,
    pub sku: String,
    pub name: String,
    pub price_cents: i64,
    pub created_at: Timestamp,
}

/// `n` rows of plausible data; the same `seed` gives the same rows.
pub fn generate(n: usize, seed: u64) -> Vec<Row> {
    let mut rng = fastrand::Rng::with_seed(seed);
    let start = Timestamp::from_second(1_767_225_600).expect("a valid timestamp"); // 2026-01-01
    (0..n)
        .map(|i| Row {
            id: i as i64 + 1,
            sku: format!("SKU{:06}", i + 1),
            // Commas and quotes, so the CSV writer has to quote: real data has them.
            name: format!(
                "Product {}, \"{}\" edition",
                i + 1,
                ["blue", "green", "red"][i % 3]
            ),
            price_cents: rng.i64(99..=99_999),
            created_at: start
                .checked_add(jiff::SignedDuration::from_secs(rng.i64(0..86_400 * 300)))
                .expect("in range"),
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Strategy {
    Insert,
    Pipelined,
    Unnest,
    CopyText,
    CopyBinary,
}

impl Strategy {
    pub const ALL: [Strategy; 5] = [
        Strategy::Insert,
        Strategy::Pipelined,
        Strategy::Unnest,
        Strategy::CopyText,
        Strategy::CopyBinary,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Strategy::Insert => "INSERT, one at a time",
            Strategy::Pipelined => "INSERT, pipelined",
            Strategy::Unnest => "INSERT … unnest, batches",
            Strategy::CopyText => "COPY, CSV",
            Strategy::CopyBinary => "COPY, binary",
        }
    }
}

/// The table every strategy loads into: temporary, and emptied at every commit.
pub const CREATE: &str = "CREATE TEMP TABLE IF NOT EXISTS load_target (
         id bigint, sku text, name text, price_cents bigint, created_at timestamptz
     ) ON COMMIT DELETE ROWS";

const INSERT: &str = "INSERT INTO load_target (id, sku, name, price_cents, created_at)
     VALUES ($1, $2, $3, $4, $5)";

/// Loads `rows` with `strategy` in one transaction, and returns how many arrived.
pub async fn load(client: &mut Client, strategy: Strategy, rows: &[Row]) -> Result<u64, Error> {
    client.batch_execute(CREATE).await?;
    let tx = client.transaction().await?;
    let loaded = load_in(&tx, strategy, rows).await?;
    tx.commit().await?; // ON COMMIT DELETE ROWS: the table is empty for the next run
    Ok(loaded)
}

/// Loads `rows` into `load_target` (which must exist: see [`CREATE`]) inside the caller's
/// transaction, and returns how many rows the table then holds.
pub async fn load_in(tx: &Transaction<'_>, strategy: Strategy, rows: &[Row]) -> Result<u64, Error> {
    match strategy {
        Strategy::Insert => insert(tx, rows).await?,
        Strategy::Pipelined => pipelined(tx, rows).await?,
        Strategy::Unnest => unnest(tx, rows).await?,
        Strategy::CopyText => copy_csv(tx, rows).await?,
        Strategy::CopyBinary => copy_binary(tx, rows).await?,
    }
    let loaded: i64 = tx
        .query_one("SELECT count(*) FROM load_target", &[])
        .await?
        .get(0);
    Ok(loaded as u64)
}

/// One round trip per row: the client waits for each answer before sending the next.
async fn insert(tx: &Transaction<'_>, rows: &[Row]) -> Result<(), Error> {
    let statement = tx.prepare(INSERT).await?;
    for r in rows {
        tx.execute(
            &statement,
            &[&r.id, &r.sku, &r.name, &r.price_cents, &r.created_at],
        )
        .await?;
    }
    Ok(())
}

/// The same statements, but up to 64 in flight: the client keeps sending while answers come
/// back (Chapter 7). On one connection, in one transaction, in order.
async fn pipelined(tx: &Transaction<'_>, rows: &[Row]) -> Result<(), Error> {
    let statement = &tx.prepare(INSERT).await?;
    stream::iter(rows)
        .map(|r| async move {
            tx.execute(
                statement,
                &[&r.id, &r.sku, &r.name, &r.price_cents, &r.created_at],
            )
            .await
        })
        .buffered(64)
        .try_for_each(|_| async { Ok(()) })
        .await
}

/// One statement per 1,000 rows: each column travels as an array, and `unnest` zips the
/// arrays back into rows on the server.
async fn unnest(tx: &Transaction<'_>, rows: &[Row]) -> Result<(), Error> {
    let statement = tx
        .prepare(
            "INSERT INTO load_target (id, sku, name, price_cents, created_at)
             SELECT * FROM unnest($1::bigint[], $2::text[], $3::text[], $4::bigint[], $5::timestamptz[])",
        )
        .await?;
    for batch in rows.chunks(1_000) {
        let ids: Vec<i64> = batch.iter().map(|r| r.id).collect();
        let skus: Vec<&str> = batch.iter().map(|r| r.sku.as_str()).collect();
        let names: Vec<&str> = batch.iter().map(|r| r.name.as_str()).collect();
        let prices: Vec<i64> = batch.iter().map(|r| r.price_cents).collect();
        let times: Vec<Timestamp> = batch.iter().map(|r| r.created_at).collect();
        tx.execute(&statement, &[&ids, &skus, &names, &prices, &times])
            .await?;
    }
    Ok(())
}

/// COPY with CSV: the rows as text, in chunks of about 64 KiB, into a `Sink`.
async fn copy_csv(tx: &Transaction<'_>, rows: &[Row]) -> Result<(), Error> {
    let sink = tx
        .copy_in("COPY load_target FROM STDIN (FORMAT csv)")
        .await?;
    let mut sink = pin!(sink);
    let mut csv = csv::Writer::from_writer(Vec::with_capacity(64 * 1024));
    for r in rows {
        let at = r.created_at.to_string();
        csv.write_record([
            &r.id.to_string(),
            &r.sku,
            &r.name,
            &r.price_cents.to_string(),
            &at,
        ])
        .expect("writing to memory cannot fail");
        if csv.get_ref().len() >= 64 * 1024 {
            let chunk = std::mem::replace(
                &mut csv,
                csv::Writer::from_writer(Vec::with_capacity(64 * 1024)),
            );
            sink.send(Bytes::from(chunk.into_inner().expect("flushing to memory")))
                .await?;
        }
    }
    sink.send(Bytes::from(csv.into_inner().expect("flushing to memory")))
        .await?;
    // Until `finish`, nothing is committed to the table; dropping the sink aborts the COPY.
    sink.as_mut().finish().await?;
    Ok(())
}

/// COPY with the binary format: no text formatting or parsing at all, but the column types
/// must be named, in order, and match the table exactly.
async fn copy_binary(tx: &Transaction<'_>, rows: &[Row]) -> Result<(), Error> {
    let sink = tx
        .copy_in("COPY load_target FROM STDIN (FORMAT binary)")
        .await?;
    let types = [
        Type::INT8,
        Type::TEXT,
        Type::TEXT,
        Type::INT8,
        Type::TIMESTAMPTZ,
    ];
    let writer = BinaryCopyInWriter::new(sink, &types);
    let mut writer = pin!(writer);
    for r in rows {
        writer
            .as_mut()
            .write(&[&r.id, &r.sku, &r.name, &r.price_cents, &r.created_at])
            .await?;
    }
    writer.as_mut().finish().await?;
    Ok(())
}

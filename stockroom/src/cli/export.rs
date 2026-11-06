//! `stockroom export`: data out, in batches from a portal or in binary from COPY (Chapters 9, 10).

use std::path::PathBuf;
use std::pin::pin;

use clap::{Args, ValueEnum};
use futures_util::TryStreamExt;
use jiff::Timestamp;
use tokio::io::{AsyncWrite, AsyncWriteExt, BufWriter};
use tokio_postgres::binary_copy::BinaryCopyOutStream;
use tokio_postgres::types::Type;
use tokio_postgres::{Client, IsolationLevel};

use crate::repo::products::{ALL_IN_SKU_ORDER, Product};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum What {
    /// The catalog, as newline-delimited JSON.
    Products,
    /// The stock ledger, as CSV.
    Movements,
}

#[derive(Debug, Clone, Args)]
pub struct ExportArgs {
    pub what: What,
    /// Where to write; `-` for standard output.
    #[arg(long, default_value = "-")]
    pub out: PathBuf,
    /// Rows per round trip (products only).
    #[arg(long, default_value_t = 500)]
    pub batch: i32,
}

pub async fn run(client: &mut Client, args: &ExportArgs) -> anyhow::Result<u64> {
    let out: Box<dyn AsyncWrite + Unpin + Send> = if args.out.as_os_str() == "-" {
        Box::new(tokio::io::stdout())
    } else {
        Box::new(tokio::fs::File::create(&args.out).await?)
    };
    let mut out = BufWriter::new(out);
    let rows = match args.what {
        What::Products => products(client, args.batch.max(1), &mut out).await?,
        What::Movements => movements(client, &mut out).await?,
    };
    out.flush().await?;
    Ok(rows)
}

/// The catalog through a portal: a query bound once and then read `batch` rows at a time,
/// so neither side ever holds the whole result.
///
/// A portal lives in a transaction. `SERIALIZABLE, READ ONLY, DEFERRABLE` is the one made
/// for exports: it waits, if needed, for a moment when a consistent snapshot is guaranteed,
/// and then can never fail with a serialization error, nor slow anyone else down.
async fn products(
    client: &mut Client,
    batch: i32,
    out: &mut (impl AsyncWrite + Unpin),
) -> anyhow::Result<u64> {
    let tx = client
        .build_transaction()
        .isolation_level(IsolationLevel::Serializable)
        .read_only(true)
        .deferrable(true)
        .start()
        .await?;
    let statement = tx.prepare(ALL_IN_SKU_ORDER).await?;
    let portal = tx.bind(&statement, &[]).await?;
    let mut total = 0;
    loop {
        let rows = tx.query_portal(&portal, batch).await?;
        for row in &rows {
            let mut line = serde_json::to_vec(&Product::try_from(row)?)?;
            line.push(b'\n');
            out.write_all(&line).await?;
        }
        total += rows.len() as u64;
        if rows.len() < batch as usize {
            break; // a short batch is the last one
        }
    }
    tx.commit().await?;
    Ok(total)
}

/// The ledger through binary COPY: the server sends every row in its binary wire format, and
/// `BinaryCopyOutStream` decodes them, given the column types in order.
async fn movements(client: &Client, out: &mut (impl AsyncWrite + Unpin)) -> anyhow::Result<u64> {
    let copy = client
        .copy_out(
            "COPY (
                 SELECT m.id, s.code, p.sku, m.delta, m.reason, m.at
                 FROM stock_movements m
                 JOIN stores s ON s.id = m.store_id
                 JOIN products p ON p.id = m.product_id
                 ORDER BY m.id
             ) TO STDOUT (FORMAT binary)",
        )
        .await?;
    // `p.sku` is a domain; in COPY it arrives as its base type.
    let rows = BinaryCopyOutStream::new(
        copy,
        &[
            Type::INT8,
            Type::TEXT,
            Type::TEXT,
            Type::INT4,
            Type::TEXT,
            Type::TIMESTAMPTZ,
        ],
    );
    let mut rows = pin!(rows);
    let mut csv = csv::Writer::from_writer(Vec::new());
    csv.write_record(["id", "store", "sku", "delta", "reason", "at"])?;
    let mut total = 0;
    while let Some(row) = rows.try_next().await? {
        let id: i64 = row.get(0);
        let store: &str = row.get(1);
        let sku: &str = row.get(2);
        let delta: i32 = row.get(3);
        let reason: &str = row.get(4);
        let at: Timestamp = row.get(5);
        csv.write_record([
            &id.to_string(),
            store,
            sku,
            &delta.to_string(),
            reason,
            &at.to_string(),
        ])?;
        total += 1;
        if total % 1_000 == 0 {
            // Hand the formatted rows over in chunks rather than holding the whole file.
            let chunk = std::mem::replace(&mut csv, csv::Writer::from_writer(Vec::new()));
            out.write_all(&chunk.into_inner().map_err(|e| e.into_error())?)
                .await?;
        }
    }
    out.write_all(&csv.into_inner().map_err(|e| e.into_error())?)
        .await?;
    Ok(total)
}

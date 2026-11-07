//! How much the mapping costs: 100,000 rows of 12 columns, read into a struct four ways.
//!
//!     cargo run -p shelf --release --example bench_mapping
//!
//! It reads `DATABASE_URL` (or `.env`), fetches the rows once, and times only the decoding:
//! by name and by index written by hand, and through shelf, resolving the columns once per
//! statement (`from_rows`) or again for every row (`from_row`). The columns come back in the
//! reverse order of the struct, so that a search by name walks half the row on average.
//! The median of five rounds is printed as a Markdown table (Chapter 5 shows it).

use std::time::{Duration, Instant};

use shelf::FromRow;
use tokio_postgres::{NoTls, Row};

#[derive(FromRow)]
#[allow(dead_code)] // only the decoding is measured
struct Wide {
    c01: i64,
    c02: String,
    c03: i32,
    c04: Option<String>,
    c05: i64,
    c06: bool,
    c07: String,
    c08: i64,
    c09: Option<i32>,
    c10: String,
    c11: i64,
    c12: f64,
}

fn by_name(r: &Row) -> Result<Wide, tokio_postgres::Error> {
    Ok(Wide {
        c01: r.try_get("c01")?,
        c02: r.try_get("c02")?,
        c03: r.try_get("c03")?,
        c04: r.try_get("c04")?,
        c05: r.try_get("c05")?,
        c06: r.try_get("c06")?,
        c07: r.try_get("c07")?,
        c08: r.try_get("c08")?,
        c09: r.try_get("c09")?,
        c10: r.try_get("c10")?,
        c11: r.try_get("c11")?,
        c12: r.try_get("c12")?,
    })
}

fn by_index(r: &Row, i: &[usize; 12]) -> Result<Wide, tokio_postgres::Error> {
    Ok(Wide {
        c01: r.try_get(i[0])?,
        c02: r.try_get(i[1])?,
        c03: r.try_get(i[2])?,
        c04: r.try_get(i[3])?,
        c05: r.try_get(i[4])?,
        c06: r.try_get(i[5])?,
        c07: r.try_get(i[6])?,
        c08: r.try_get(i[7])?,
        c09: r.try_get(i[8])?,
        c10: r.try_get(i[9])?,
        c11: r.try_get(i[10])?,
        c12: r.try_get(i[11])?,
    })
}

const SQL: &str = "
    SELECT (g * 7)::float8 AS c12, g::bigint AS c11, 'ten ' || g AS c10,
           CASE WHEN g % 3 = 0 THEN NULL ELSE g END AS c09, g::bigint AS c08, 'seven' AS c07,
           g % 2 = 0 AS c06, g::bigint AS c05, CASE WHEN g % 5 = 0 THEN NULL ELSE 'four' END AS c04,
           g AS c03, 'sku-' || g AS c02, g::bigint AS c01
    FROM generate_series(1, 100000) AS g";

fn median(mut times: Vec<Duration>) -> f64 {
    times.sort();
    times[times.len() / 2].as_secs_f64() * 1e3
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();
    let url = std::env::var("DATABASE_URL").expect("set DATABASE_URL (or put it in .env)");
    let (client, connection) = tokio_postgres::connect(&url, NoTls).await?;
    tokio::spawn(connection);
    let rows = client.query(SQL, &[]).await?;
    let names = [
        "c01", "c02", "c03", "c04", "c05", "c06", "c07", "c08", "c09", "c10", "c11", "c12",
    ];
    let columns = rows[0].columns();
    let index: [usize; 12] =
        std::array::from_fn(|k| columns.iter().position(|c| c.name() == names[k]).unwrap());

    let mut results: [(&str, Vec<Duration>); 4] = [
        ("by name, by hand (`try_get(\"c01\")`)", Vec::new()),
        ("by index, by hand (`try_get(i)`)", Vec::new()),
        ("shelf, resolved once (`from_rows`)", Vec::new()),
        ("shelf, resolved per row (`from_row`)", Vec::new()),
    ];
    for _ in 0..5 {
        let t = Instant::now();
        let a = rows.iter().map(by_name).collect::<Result<Vec<_>, _>>()?;
        results[0].1.push(t.elapsed());
        let t = Instant::now();
        let b = rows
            .iter()
            .map(|r| by_index(r, &index))
            .collect::<Result<Vec<_>, _>>()?;
        results[1].1.push(t.elapsed());
        let t = Instant::now();
        let c: Vec<Wide> = shelf::from_rows(&rows)?;
        results[2].1.push(t.elapsed());
        let t = Instant::now();
        let d = rows
            .iter()
            .map(Wide::from_row)
            .collect::<Result<Vec<_>, _>>()?;
        results[3].1.push(t.elapsed());
        assert_eq!(a.len() + b.len() + c.len() + d.len(), 4 * rows.len());
    }

    let fastest = results
        .iter()
        .map(|(_, t)| median(t.clone()))
        .fold(f64::INFINITY, f64::min);
    println!(
        "| Mapping, {} rows × 12 columns | Median of 5 | × |",
        rows.len()
    );
    println!("|---|---:|---:|");
    for (name, times) in results {
        let ms = median(times);
        println!("| {name} | {ms:.1} ms | {:.1} |", ms / fastest);
    }
    Ok(())
}

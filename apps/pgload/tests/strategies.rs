//! Every strategy stores exactly the same rows. DATABASE_URL from the environment or `.env`.

use pgload::Strategy;
use tokio_postgres::NoTls;

#[tokio::test]
async fn every_strategy_stores_the_same_rows() {
    let _ = dotenvy::dotenv();
    let url = std::env::var("DATABASE_URL").expect("set DATABASE_URL (see .env.example)");
    let (mut client, connection) = tokio_postgres::connect(&url, NoTls).await.unwrap();
    tokio::spawn(connection);
    client.batch_execute(pgload::CREATE).await.unwrap();

    // Commas and quotes in the names: the CSV path must quote them correctly.
    let rows = pgload::generate(2_500, 7);
    let mut fingerprints = Vec::new();
    for strategy in Strategy::ALL {
        let tx = client.transaction().await.unwrap();
        assert_eq!(
            pgload::load_in(&tx, strategy, &rows).await.unwrap(),
            2_500,
            "{strategy:?}"
        );
        let fingerprint: String = tx
            .query_one(
                "SELECT md5(string_agg(format('%s|%s|%s|%s|%s', id, sku, name, price_cents, created_at),
                                       ',' ORDER BY id))
                 FROM load_target",
                &[],
            )
            .await
            .unwrap()
            .get(0);
        tx.rollback().await.unwrap();
        fingerprints.push((strategy, fingerprint));
    }
    let first = &fingerprints[0].1;
    for (strategy, fingerprint) in &fingerprints {
        assert_eq!(fingerprint, first, "{strategy:?} stored different rows");
    }

    // And `load` itself commits and leaves the table empty for the next run.
    assert_eq!(
        pgload::load(&mut client, Strategy::CopyBinary, &rows)
            .await
            .unwrap(),
        2_500
    );
    let left: i64 = client
        .query_one("SELECT count(*) FROM load_target", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(left, 0);
}

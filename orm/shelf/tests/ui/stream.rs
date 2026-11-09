// A stream borrows the connection it reads from: a pooled connection cannot go back to the pool
// while rows may still be on their way.

#[derive(shelf::FromRow)]
struct Number {
    n: i64,
}

async fn first_number(pool: deadpool_postgres::Pool) -> i64 {
    let client = pool.get().await.unwrap();
    let mut rows = shelf::fetch_stream::<Number>(&client, "SELECT 1::bigint AS n", &[])
        .await
        .unwrap();
    drop(client); // back to the pool, with the stream unfinished
    let first = futures_util::TryStreamExt::try_next(&mut rows).await.unwrap();
    first.unwrap().n
}

fn main() {
    let _ = first_number;
}

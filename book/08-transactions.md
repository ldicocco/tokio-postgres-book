# Chapter 8 — Transactions and concurrency control

> **Async PostgreSQL in Rust** · Previous: [Chapter 7 — One connection, many tasks: pipelining](07-pipelining.md) ·
> Next: [Chapter 9 — Streaming rows and portals](09-streaming.md)

A store has five garden chairs and twenty customers who want one, all clicking "buy" in the
same second. A program that reads "five available", decides "fine, one more", and writes "one
less" will sell more than five chairs, every time, because twenty copies of it read "five"
before any of them wrote. That is the problem this chapter solves, and the tools are
PostgreSQL's: transactions, isolation levels, row locks and constraints.

tokio-postgres's part is small but strict. `Transaction` borrows the client mutably, so the
compiler enforces what Chapter 7 showed the hard way: nothing else can run on a connection
while a transaction owns it. The chapter covers transactions and that borrow, isolation
levels, savepoints, writing functions that work inside or outside a transaction, retrying
transactions that lose a race, locks, and optimistic concurrency. It ends by solving the
chairs problem four ways, and proving each one with twenty concurrent customers.

---

## 8.1 `transaction`, `commit`, and rollback on drop

`Client::transaction` sends `BEGIN` and returns a `Transaction`. It has the same query methods
as a `Client`. `commit` ends it successfully. Anything else ends it with a rollback: an explicit
`rollback`, or simply dropping it, which includes returning early with `?`:

```rust
use tokio_postgres::{Client, Error};

/// Moves money between two accounts: both updates, or neither.
async fn transfer(client: &mut Client, from: &str, to: &str, cents: i64) -> Result<(), Error> {
    let tx = client.transaction().await?;
    tx.execute(
        "UPDATE accounts SET cents = cents - $2 WHERE name = $1",
        &[&from, &cents],
    )
    .await?; // an error here returns, drops `tx`, and so rolls back
    tx.execute(
        "UPDATE accounts SET cents = cents + $2 WHERE name = $1",
        &[&to, &cents],
    )
    .await?;
    tx.commit().await
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let mut client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE accounts (name text PRIMARY KEY, cents bigint CHECK (cents >= 0));
             INSERT INTO accounts VALUES ('ada', 100), ('bob', 0);",
        )
        .await?;

    transfer(&mut client, "ada", "bob", 30).await?;
    // Too much: the first UPDATE breaks the CHECK, and nothing changes.
    assert!(transfer(&mut client, "ada", "bob", 500).await.is_err());

    // A transaction that is dropped without commit is rolled back.
    {
        let tx = client.transaction().await?;
        tx.execute("DELETE FROM accounts", &[]).await?;
    }

    let balances: Vec<(String, i64)> = client
        .query("SELECT name, cents FROM accounts ORDER BY name", &[])
        .await?
        .iter()
        .map(|row| (row.get(0), row.get(1)))
        .collect();
    assert_eq!(balances, [("ada".into(), 70), ("bob".into(), 30)]);
    Ok(())
}
```

Rollback on drop is a safety net, not a way to report errors. A `Drop` implementation cannot
wait, so the dropped transaction only *queues* a `ROLLBACK` on the connection, and nobody
hears whether it worked. Call `rollback().await` when you want to know, as stockroom's retry
helper does (§8.6).

## 8.2 Why `transaction` takes `&mut self`

A transaction is a state of the *session*. Between `BEGIN` and `COMMIT`, every statement on the
connection belongs to it, whoever sent it. Chapter 7 showed what happens when another task's
statement slips into a transaction started with plain SQL on a shared client: it is rolled back
with it. tokio-postgres prevents that at compile time. `transaction()` borrows the client
mutably, and the `Transaction` keeps that borrow until it is committed, rolled back or dropped:

~~~rust
let tx = client.transaction().await?;
client.query("SELECT 1", &[]).await?;   // the client is busy with `tx`
tx.commit().await?;
// error[E0502]: cannot borrow `client` as immutable because it is also borrowed as mutable
// note: mutable borrow might be used here, when `tx` is dropped and runs the `Drop` code
//       for type `Transaction`
~~~

And a client behind an `Arc`, shared by many tasks, cannot start a transaction at all, because
nobody can get `&mut` through an `Arc`:

~~~rust
let shared = Arc::new(client);
let tx = shared.transaction().await?;
// error[E0596]: cannot borrow data in an `Arc` as mutable
~~~

That is the design working, not getting in the way: a transaction needs a connection of its
own for as long as it runs. A program with concurrent transactions therefore needs several
connections, and handing them out is a pool's job (Chapter 13). The examples below that need
concurrency simply open one connection per task.

## 8.3 Isolation levels, read-only and deferrable

What a transaction sees of other transactions' changes is its *isolation level*.
`build_transaction` chooses it, along with two more options, before sending `BEGIN`:

~~~rust
let tx = client
    .build_transaction()
    .isolation_level(IsolationLevel::RepeatableRead) // ReadCommitted is the default
    .read_only(true)                                 // writes fail with 25006
    .deferrable(true)                                // only matters for SERIALIZABLE READ ONLY
    .start()
    .await?;
~~~

> **SQL lens:** PostgreSQL never lets a reader see uncommitted changes, and readers never block
> writers or the other way round (that is MVCC: every statement reads a *snapshot*). The levels
> differ in when the snapshot is taken and what is checked at commit.
> **READ COMMITTED** (the default) takes a new snapshot for every *statement*, so a transaction
> sees changes that others committed while it was running. **REPEATABLE READ** takes one
> snapshot for the whole *transaction*: everything it reads is as of its first statement, and
> if it tries to update a row that someone else changed since, it fails with a serialization
> error. **SERIALIZABLE** adds a check that the combined result of all committed transactions
> is one that running them one at a time could have produced, and fails one of them (40001)
> when that is not true. `READ UNCOMMITTED` exists in name only: PostgreSQL treats it as
> READ COMMITTED.

The difference is easy to see with two connections, one reading in a transaction while the
other changes the row in between. The table must be a real one, visible to both sessions,
so the example makes a schema of its own and drops it at the end:

```rust
use tokio_postgres::IsolationLevel;
use tokio_postgres::error::SqlState;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let mut reader = playground::connect().await?;
    let writer = playground::connect().await?;
    writer
        .batch_execute(
            "DROP SCHEMA IF EXISTS ch08_isolation CASCADE;
             CREATE SCHEMA ch08_isolation;
             CREATE TABLE ch08_isolation.stock (sku text PRIMARY KEY, on_hand int);
             INSERT INTO ch08_isolation.stock VALUES ('CHAIR-01', 10);",
        )
        .await?;
    let read = "SELECT on_hand FROM ch08_isolation.stock WHERE sku = 'CHAIR-01'";
    let set = "UPDATE ch08_isolation.stock SET on_hand = $1 WHERE sku = 'CHAIR-01'";

    for (level, second_read) in [
        (IsolationLevel::ReadCommitted, 7), // sees the other connection's commit
        (IsolationLevel::RepeatableRead, 10), // keeps its snapshot
    ] {
        writer.execute(set, &[&10]).await?;
        let tx = reader
            .build_transaction()
            .isolation_level(level)
            .start()
            .await?;
        let first: i32 = tx.query_one(read, &[]).await?.get(0);
        writer.execute(set, &[&7]).await?; // commits at once: no transaction
        let second: i32 = tx.query_one(read, &[]).await?.get(0);
        tx.commit().await?;
        assert_eq!((first, second), (10, second_read), "{level:?}");
    }

    // A read-only transaction refuses to write.
    let tx = reader.build_transaction().read_only(true).start().await?;
    let error = tx.execute(set, &[&1]).await.unwrap_err();
    assert_eq!(error.code(), Some(&SqlState::READ_ONLY_SQL_TRANSACTION)); // 25006
    drop(tx);

    writer
        .batch_execute("DROP SCHEMA ch08_isolation CASCADE")
        .await?;
    Ok(())
}
```

`deferrable(true)` has an effect only together with `SERIALIZABLE` and `read_only(true)`: the
transaction may wait at its start until it can take a snapshot that is guaranteed to be
consistent, and from then on it can never fail with a serialization error, nor make anyone
else's fail. That is the right setting for long reports and exports, and stockroom's `export`
command uses it.

## 8.4 Savepoints: transactions inside transactions

`Transaction` has a `transaction()` method of its own, and `savepoint(name)`, which does the
same with a name of your choice. Both start a *nested* transaction with `SAVEPOINT`.
Committing it (`RELEASE SAVEPOINT`) keeps its changes as part of the outer transaction, and
rolling it back (`ROLLBACK TO SAVEPOINT`) undoes only what happened since the savepoint. Either
way, the outer transaction carries on, healthy:

```rust
#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let mut client = playground::connect().await?;
    client
        .batch_execute("CREATE TEMP TABLE products (sku text PRIMARY KEY)")
        .await?;

    let mut tx = client.transaction().await?;
    tx.execute("INSERT INTO products VALUES ('CHAIR-01')", &[])
        .await?;
    for sku in ["LAMP-01", "CHAIR-01", "VASE-01"] {
        // One savepoint per item: a failed insert spoils only its own savepoint.
        let item = tx.savepoint("item").await?;
        match item
            .execute("INSERT INTO products VALUES ($1)", &[&sku])
            .await
        {
            Ok(_) => item.commit().await?,
            Err(_) => item.rollback().await?, // the duplicate CHAIR-01
        }
    }
    // Nested transactions nest further, and their commit is still provisional:
    {
        let mut inner = tx.transaction().await?;
        let innermost = inner.transaction().await?;
        innermost
            .execute("INSERT INTO products VALUES ('RUG-01')", &[])
            .await?;
        innermost.commit().await?;
        // `inner` is dropped without commit: RUG-01 goes with it.
    }
    tx.commit().await?;

    let skus: Vec<String> = client
        .query_scalar("SELECT sku FROM products ORDER BY sku", &[])
        .await?;
    assert_eq!(skus, ["CHAIR-01", "LAMP-01", "VASE-01"]);
    Ok(())
}
```

Savepoints are cheap, but not free: each one is a subtransaction on the server, and a very
large number of them in one transaction (tens of thousands) has a measurable cost. One per
item in a batch of a few hundred is fine, and it is exactly how stockroom's batch endpoint
keeps the good items when some fail (Chapter 6).

## 8.5 `GenericClient`: code that runs inside or outside a transaction

Most data-access functions do not care whether they run inside a transaction. Writing them
against `&Client` locks them out of transactions, and writing them against `&Transaction`
locks them in. `GenericClient` is the trait both implement, with the query methods they share:

```rust
use tokio_postgres::{Error, GenericClient};

/// Works on a plain client and inside a transaction alike.
async fn stock_of(db: &impl GenericClient, sku: &str) -> Result<i32, Error> {
    let row = db
        .query_one("SELECT on_hand FROM stock WHERE sku = $1", &[&sku])
        .await?;
    Ok(row.get(0))
}

async fn receive(db: &impl GenericClient, sku: &str, units: i32) -> Result<(), Error> {
    db.execute(
        "UPDATE stock SET on_hand = on_hand + $2 WHERE sku = $1",
        &[&sku, &units],
    )
    .await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let mut client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE stock (sku text PRIMARY KEY, on_hand int);
             INSERT INTO stock VALUES ('CHAIR-01', 5);",
        )
        .await?;

    assert_eq!(stock_of(&client, "CHAIR-01").await?, 5); // a Client

    let tx = client.transaction().await?;
    receive(&tx, "CHAIR-01", 3).await?; // a Transaction
    assert_eq!(stock_of(&tx, "CHAIR-01").await?, 8);
    tx.rollback().await?;

    assert_eq!(stock_of(&client, "CHAIR-01").await?, 5);
    Ok(())
}
```

Three things to know about it. It is *sealed*: only tokio-postgres can implement it, so you
cannot add your own client type. Its async methods are written with the `async-trait` style
and return boxed `Send` futures, which is why generic functions over it spawn without trouble.
And it is not object-safe, so use it as a generic bound (`&impl GenericClient`), not as
`&dyn GenericClient`. Pools have their own version: deadpool-postgres's `GenericClient` adds
`prepare_cached`, and stockroom's repository functions use that one (Chapter 14).

## 8.6 Retrying transactions that lose a race

At REPEATABLE READ and SERIALIZABLE, PostgreSQL resolves conflicts between concurrent
transactions by failing one of them with SQLSTATE 40001, *serialization failure*. Deadlocks are
resolved the same way at any level, with 40P01. Neither error says anything is wrong with the
transaction itself: run it again, from `BEGIN`, and it will most likely succeed.

Here is a conflict that only SERIALIZABLE catches. Two doctors are on call, and the rule is
that at least one must stay on call. Two transactions each check the rule and then take one
doctor off; each change is fine on its own, but together they break the rule (a *write skew*):

```rust
use tokio_postgres::IsolationLevel;
use tokio_postgres::error::SqlState;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let mut first = playground::connect().await?;
    let mut second = playground::connect().await?;
    first
        .batch_execute(
            "DROP SCHEMA IF EXISTS ch08_oncall CASCADE; CREATE SCHEMA ch08_oncall;
             CREATE TABLE ch08_oncall.doctors (name text PRIMARY KEY, on_call bool);
             INSERT INTO ch08_oncall.doctors VALUES ('alice', true), ('bob', true);",
        )
        .await?;
    let count = "SELECT count(*) FROM ch08_oncall.doctors WHERE on_call";
    let leave = "UPDATE ch08_oncall.doctors SET on_call = false WHERE name = $1";

    let a = first
        .build_transaction()
        .isolation_level(IsolationLevel::Serializable)
        .start()
        .await?;
    let b = second
        .build_transaction()
        .isolation_level(IsolationLevel::Serializable)
        .start()
        .await?;
    // Both check the rule: two on call, so one may leave.
    assert_eq!(a.query_one(count, &[]).await?.get::<_, i64>(0), 2);
    assert_eq!(b.query_one(count, &[]).await?.get::<_, i64>(0), 2);
    a.execute(leave, &[&"alice"]).await?;
    b.execute(leave, &[&"bob"]).await?;
    a.commit().await?;
    // Committing b too would leave nobody on call. SERIALIZABLE notices.
    let error = b.commit().await.unwrap_err();
    assert_eq!(error.code(), Some(&SqlState::T_R_SERIALIZATION_FAILURE));
    println!("{}", error.as_db_error().unwrap().message());

    first
        .batch_execute("DROP SCHEMA ch08_oncall CASCADE")
        .await?;
    Ok(())
}
```

```text
could not serialize access due to read/write dependencies among transactions
```

At READ COMMITTED, both commits succeed and nobody is on call. At SERIALIZABLE, the second
transaction fails, and the right reaction is to run it again: its new check sees one doctor on
call, and it declines. Code that uses SERIALIZABLE must therefore be ready to retry, which
calls for a helper: run the transaction, and on 40001 or 40P01, wait a little and start over.

Modern Rust writes the "transaction body" as an *async closure* (`async |tx| …`, stable since
Rust 1.85), which the helper calls once per attempt with that attempt's transaction:

```rust
use std::time::Duration;
use tokio_postgres::error::SqlState;
use tokio_postgres::{Client, Error, IsolationLevel, Transaction};

/// Runs `work` in a SERIALIZABLE transaction, again from the start after a serialization
/// failure or a deadlock, up to 10 times.
async fn serializable<T>(
    client: &mut Client,
    mut work: impl AsyncFnMut(&Transaction<'_>) -> Result<T, Error>,
) -> Result<T, Error> {
    let mut attempt = 1;
    loop {
        let tx = client
            .build_transaction()
            .isolation_level(IsolationLevel::Serializable)
            .start()
            .await?;
        let outcome = match work(&tx).await {
            Ok(value) => tx.commit().await.map(|()| value),
            Err(error) => Err(error), // `tx` is dropped here: rolled back
        };
        match outcome {
            Err(e)
                if attempt < 10
                    && matches!(
                        e.code(),
                        Some(
                            &SqlState::T_R_SERIALIZATION_FAILURE | &SqlState::T_R_DEADLOCK_DETECTED
                        )
                    ) =>
            {
                // A random pause, so that the losers do not collide again in lockstep.
                tokio::time::sleep(Duration::from_millis(fastrand_ms(attempt))).await;
                attempt += 1;
            }
            other => return other,
        }
    }
}

/// Up to 2, 4, 8 … ms, capped at 100, at random (no extra crate needed for an example).
fn fastrand_ms(attempt: u32) -> u64 {
    let ceiling = (2u64 << attempt).min(100);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos() as u64;
    1 + nanos % ceiling
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let setup = playground::connect().await?;
    setup
        .batch_execute(
            "DROP SCHEMA IF EXISTS ch08_retry CASCADE; CREATE SCHEMA ch08_retry;
             CREATE TABLE ch08_retry.doctors (name text PRIMARY KEY, on_call bool);
             INSERT INTO ch08_retry.doctors VALUES ('alice', true), ('bob', true);",
        )
        .await?;

    // Both doctors try to leave at the same time, each in a task of its own.
    let mut tasks = Vec::new();
    for name in ["alice", "bob"] {
        let name = name.to_owned(); // owned: see below
        tasks.push(tokio::spawn(async move {
            let mut client = playground::connect().await?;
            // `async move`: the closure owns what it uses; every attempt borrows from it.
            serializable(&mut client, async move |tx| {
                let on_call: i64 = tx
                    .query_one("SELECT count(*) FROM ch08_retry.doctors WHERE on_call", &[])
                    .await?
                    .get(0);
                if on_call < 2 {
                    return Ok(false); // someone must stay
                }
                tokio::time::sleep(Duration::from_millis(20)).await; // a slow decision
                tx.execute(
                    "UPDATE ch08_retry.doctors SET on_call = false WHERE name = $1",
                    &[&name],
                )
                .await?;
                Ok(true)
            })
            .await
        }));
    }
    let mut left = 0;
    for task in tasks {
        if task.await.expect("no panic")? {
            left += 1;
        }
    }
    assert_eq!(left, 1, "exactly one doctor left");
    let still: i64 = setup
        .query_one("SELECT count(*) FROM ch08_retry.doctors WHERE on_call", &[])
        .await?
        .get(0);
    assert_eq!(still, 1);
    setup
        .batch_execute("DROP SCHEMA ch08_retry CASCADE")
        .await?;
    Ok(())
}
```

Two rules make such a helper safe.

**The body must be safe to run more than once.** Everything it does to the database is rolled
back with the failed attempt, but anything it does *outside* (sending an email, calling
another service, incrementing a counter in memory) happens once per attempt. Keep side effects
out of transaction bodies, or record them in the database and act on them after the commit:
the outbox pattern (Chapter 16).

**The closure must own what it uses, with no references inside.** Note the `async move` in
`main`, and the `to_owned()` before it: the closure takes ownership of `name` as a `String` (an
owned request struct, in a web handler), and each attempt's future borrows from the closure.
Had the closure borrowed a local variable of the task instead, the program would not compile.
Surprisingly, neither does capturing the original `name`, a `&'static str`: any reference in the
captures trips the same check:

~~~rust
let request = load_request();
tokio::spawn(async move {
    serializable(&mut client, async |tx| apply(tx, &request).await).await
    //                        ^^^^^ borrows `request` from the enclosing scope
});
// error: implementation of `Send` is not general enough
~~~

> **Rust refresher:** `AsyncFnMut(&Transaction<'_>) -> T` is a bound on an async closure that
> can be called repeatedly, each call borrowing a transaction for some lifetime of its own,
> and returning a future that may borrow both the transaction and the closure's captured state.
> The compiler must prove that this future is `Send` *for every possible lifetime* before
> `tokio::spawn` (or axum) accepts it. With owned captures it can; when the captures contain a
> reference (to the caller's locals, or even a `&'static str`), today's compiler cannot, and
> reports "not general enough", with a note about "`Send` … for any lifetime `'0`". The fix is
> to move owned data into the closure, as above. In older code you will also see the
> pre-async-closure form, `for<'t> FnMut(&'t Transaction<'t>) -> BoxFuture<'t, Result<T>>` with
> `Box::pin(async move { … })` at each call site, which states `Send` explicitly.

## 8.7 Locks

Isolation decides what a transaction *sees*. Locks decide who *waits*. An `UPDATE` or `DELETE`
locks the rows it changes until the transaction ends, and a `SELECT` can take the same lock
before it decides anything:

| Clause | Meaning |
|---|---|
| `FOR UPDATE` | Lock the selected rows as if about to update them; others wait to update or lock them |
| `FOR NO KEY UPDATE` | The same, but foreign-key checks from other tables do not wait (use it when you will not change the key) |
| `… NOWAIT` | Do not wait for a locked row: fail at once with 55P03 |
| `… SKIP LOCKED` | Do not wait for a locked row: leave it out of the result |

`SET LOCAL lock_timeout = '100ms'` bounds the wait for the rest of the transaction (55P03 again
when it runs out). *Advisory locks* lock a number you choose instead of a row, for
coordination that has no row to lock: `pg_advisory_xact_lock(key)` waits, and
`pg_try_advisory_xact_lock(key)` returns `false` instead of waiting. Both are released at the
end of the transaction.

```rust
use tokio_postgres::error::SqlState;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let mut a = playground::connect().await?;
    let mut b = playground::connect().await?;
    a.batch_execute(
        "DROP SCHEMA IF EXISTS ch08_locks CASCADE; CREATE SCHEMA ch08_locks;
         CREATE TABLE ch08_locks.jobs (id int PRIMARY KEY, done bool DEFAULT false);
         INSERT INTO ch08_locks.jobs (id) SELECT generate_series(1, 6);",
    )
    .await?;
    let claim = "SELECT id FROM ch08_locks.jobs WHERE NOT done ORDER BY id LIMIT 2
                 FOR UPDATE SKIP LOCKED";

    let worker_a = a.transaction().await?;
    let worker_b = b.transaction().await?;
    // Two workers claim jobs at the same time: SKIP LOCKED gives them different ones.
    let ids =
        |rows: Vec<tokio_postgres::Row>| -> Vec<i32> { rows.iter().map(|r| r.get(0)).collect() };
    let mine = ids(worker_a.query(claim, &[]).await?);
    let theirs = ids(worker_b.query(claim, &[]).await?);
    assert_eq!((mine, theirs), (vec![1, 2], vec![3, 4]));

    // NOWAIT on a row someone holds: an error now, instead of a wait.
    let error = worker_b
        .query(
            "SELECT * FROM ch08_locks.jobs WHERE id = 1 FOR UPDATE NOWAIT",
            &[],
        )
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some(&SqlState::LOCK_NOT_AVAILABLE)); // 55P03
    worker_b.rollback().await?;

    // An advisory lock: a number, not a row. The second taker is told no.
    let locked: bool = worker_a
        .query_one("SELECT pg_try_advisory_xact_lock(42)", &[])
        .await?
        .get(0);
    let also: bool = b
        .query_one("SELECT pg_try_advisory_xact_lock(42)", &[])
        .await?
        .get(0);
    assert!(locked && !also);
    worker_a.commit().await?; // releases the row locks and the advisory lock

    a.batch_execute("DROP SCHEMA ch08_locks CASCADE").await?;
    Ok(())
}
```

`FOR UPDATE SKIP LOCKED` is how a table becomes a job queue that several workers can share
without stepping on each other: each worker claims a few rows, and the rows stay claimed exactly
as long as its transaction is open. stockroom's outbox worker is built on it (Chapter 16).

> **Pitfall:** locks are held until the transaction ends, so a transaction that waits on
> something slow outside the database (an HTTP call, a user) holds its locks, and its
> connection, the whole time. Keep transactions short, and set
> `idle_in_transaction_session_timeout` on the server as a backstop for the ones that are not.

## 8.8 Optimistic concurrency: version numbers

Locks are *pessimistic*: they assume a conflict and make others wait. For a change that a user
prepares slowly (an edit form open for minutes), holding a lock is out of the question.
*Optimistic* concurrency assumes there will be no conflict, and checks at the end: every row
carries a version number, an update only applies if the version is still the one the client
read, and the update increments it.

```rust
#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (sku text PRIMARY KEY, price_cents bigint,
                                         version int NOT NULL DEFAULT 1);
             INSERT INTO products VALUES ('CHAIR-01', 4999);",
        )
        .await?;
    let update = "UPDATE products SET price_cents = $2, version = version + 1
                  WHERE sku = $1 AND version = $3
                  RETURNING version";

    // Two editors read the product at version 1.
    let seen = 1_i32;
    // The first one saves: version 1 is current, so it applies and becomes version 2.
    let saved = client
        .query_opt(update, &[&"CHAIR-01", &3999_i64, &seen])
        .await?;
    assert_eq!(saved.map(|row| row.get::<_, i32>(0)), Some(2));
    // The second one saves what it saw at version 1: nothing matches, nothing changes.
    let lost = client
        .query_opt(update, &[&"CHAIR-01", &2999_i64, &seen])
        .await?;
    assert!(
        lost.is_none(),
        "a conflict, reported instead of overwriting"
    );

    let price: i64 = client
        .query_one("SELECT price_cents FROM products", &[])
        .await?
        .get(0);
    assert_eq!(price, 3999);
    Ok(())
}
```

Zero rows affected is the conflict signal, and the caller decides what to do: reload and show
the user what changed. Over HTTP the version becomes an ETag and the check an `If-Match`
header (Chapter 15).

## 8.9 Selling the last chair, four ways

Back to the five chairs and twenty customers. Each strategy below runs twenty concurrent tasks,
each with its own connection, all started at the same moment, and counts what was sold.

```rust
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Barrier;
use tokio_postgres::error::SqlState;
use tokio_postgres::{Client, Error, IsolationLevel};

const CUSTOMERS: usize = 20;
const CHAIRS: i32 = 5;

#[derive(Clone, Copy, Debug)]
enum Strategy {
    ReadThenWrite, // the bug
    GuardedUpdate,
    ForUpdate,
    SerializableRetry,
}

/// One customer buys one chair; returns whether they got it.
async fn buy(client: &mut Client, strategy: Strategy) -> Result<bool, Error> {
    let available = "SELECT on_hand - reserved FROM ch08_chairs.stock";
    let reserve = "UPDATE ch08_chairs.stock SET reserved = reserved + 1";
    match strategy {
        Strategy::ReadThenWrite => {
            let left: i32 = client.query_one(available, &[]).await?.get(0);
            tokio::time::sleep(Duration::from_millis(20)).await; // thinking…
            if left < 1 {
                return Ok(false);
            }
            client.execute(reserve, &[]).await?;
            Ok(true)
        }
        Strategy::GuardedUpdate => {
            // The check and the change in one statement. A concurrent update makes this one
            // wait, then re-check the condition against the row as the other one left it.
            let changed = client
                .execute(
                    "UPDATE ch08_chairs.stock SET reserved = reserved + 1
                     WHERE on_hand - reserved >= 1",
                    &[],
                )
                .await?;
            Ok(changed == 1)
        }
        Strategy::ForUpdate => {
            let tx = client.transaction().await?;
            // Lock first: everyone else queues here until this transaction ends.
            let left: i32 = tx
                .query_one(&format!("{available} FOR UPDATE"), &[])
                .await?
                .get(0);
            tokio::time::sleep(Duration::from_millis(20)).await;
            if left < 1 {
                return Ok(false); // dropping `tx` rolls back and releases the lock
            }
            tx.execute(reserve, &[]).await?;
            tx.commit().await?;
            Ok(true)
        }
        Strategy::SerializableRetry => loop {
            let tx = client
                .build_transaction()
                .isolation_level(IsolationLevel::Serializable)
                .start()
                .await?;
            let attempt: Result<bool, Error> = async {
                let left: i32 = tx.query_one(available, &[]).await?.get(0);
                if left < 1 {
                    return Ok(false);
                }
                tx.execute(reserve, &[]).await?;
                Ok(true)
            }
            .await;
            match attempt {
                Ok(bought) => match tx.commit().await {
                    Ok(()) => return Ok(bought),
                    Err(e) if e.code() == Some(&SqlState::T_R_SERIALIZATION_FAILURE) => {}
                    Err(e) => return Err(e),
                },
                Err(e) if e.code() == Some(&SqlState::T_R_SERIALIZATION_FAILURE) => {}
                Err(e) => return Err(e),
            }
            // Lost the race: try again from BEGIN, after a short pause.
            tokio::time::sleep(Duration::from_millis(2)).await;
        },
    }
}

async fn run(strategy: Strategy) -> Result<(i64, i32), Error> {
    let setup = playground::connect().await?;
    setup
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS ch08_chairs CASCADE; CREATE SCHEMA ch08_chairs;
             CREATE TABLE ch08_chairs.stock (on_hand int, reserved int);
             INSERT INTO ch08_chairs.stock VALUES ({CHAIRS}, 0);"
        ))
        .await?;
    // Connect everyone first, then start them all at the same moment.
    let start = Arc::new(Barrier::new(CUSTOMERS));
    let mut customers = Vec::new();
    for _ in 0..CUSTOMERS {
        let mut client = playground::connect().await?;
        let start = Arc::clone(&start);
        customers.push(tokio::spawn(async move {
            start.wait().await;
            buy(&mut client, strategy).await
        }));
    }
    let mut sold = 0;
    for customer in customers {
        if customer.await.expect("no panic")? {
            sold += 1;
        }
    }
    let reserved: i32 = setup
        .query_one("SELECT reserved FROM ch08_chairs.stock", &[])
        .await?
        .get(0);
    setup
        .batch_execute("DROP SCHEMA ch08_chairs CASCADE")
        .await?;
    Ok((sold, reserved))
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let (sold, reserved) = run(Strategy::ReadThenWrite).await?;
    println!("read, then write: sold {sold} of {CHAIRS}");
    assert!(sold > CHAIRS as i64 && reserved > CHAIRS, "oversold");

    for strategy in [
        Strategy::GuardedUpdate,
        Strategy::ForUpdate,
        Strategy::SerializableRetry,
    ] {
        let (sold, reserved) = run(strategy).await?;
        println!("{strategy:?}: sold {sold} of {CHAIRS}");
        assert_eq!((sold, reserved), (CHAIRS as i64, CHAIRS), "{strategy:?}");
    }
    Ok(())
}
```

```text
read, then write: sold 20 of 5
GuardedUpdate: sold 5 of 5
ForUpdate: sold 5 of 5
SerializableRetry: sold 5 of 5
```

Twenty chairs sold out of five: every customer read "five left" before anyone wrote. The three
correct strategies have different costs:

| Strategy | How it is correct | Cost |
|---|---|---|
| **Guarded update** | The condition and the change are one statement; after waiting for a row lock, PostgreSQL re-checks the condition against the newest row version | One statement, no retries. The best choice whenever the decision fits in a `WHERE` clause |
| **`SELECT … FOR UPDATE`** | Everyone queues on the row lock; the check runs with the lock held | Customers wait for each other, one at a time, for as long as each holds the lock |
| **SERIALIZABLE + retry** | Any interleaving that is not equivalent to a serial order fails, and is run again | Retries; the right tool when the rule spans several rows or tables and fits no `WHERE` clause |

A `CHECK (reserved <= on_hand)` constraint on the table is the safety net under all of them: if
the code is ever wrong, the database refuses the bad row instead of storing it. stockroom has
that constraint, and still uses the guarded update.

### Side by side — a transaction

The task: move 30 cents between two accounts, both changes or neither.

```sql
-- psql
tpbook=# BEGIN;
BEGIN
tpbook=*# UPDATE accounts SET cents = cents - 30 WHERE name = 'ada';
UPDATE 1
tpbook=*# UPDATE accounts SET cents = cents + 30 WHERE name = 'bob';
UPDATE 1
tpbook=*# COMMIT;
COMMIT
```

~~~rust
// sqlx (illustration)
let mut tx = pool.begin().await?; // a connection from the pool, with BEGIN sent
sqlx::query("UPDATE accounts SET cents = cents - $1 WHERE name = $2").bind(30_i64).bind("ada").execute(&mut *tx).await?;
sqlx::query("UPDATE accounts SET cents = cents + $1 WHERE name = $2").bind(30_i64).bind("bob").execute(&mut *tx).await?;
tx.commit().await?; // dropped without commit: rolled back
~~~

```rust
#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let mut client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE accounts (name text PRIMARY KEY, cents bigint);
             INSERT INTO accounts VALUES ('ada', 100), ('bob', 0);",
        )
        .await?;
    let tx = client.transaction().await?;
    let move_money = "UPDATE accounts SET cents = cents + $2 WHERE name = $1";
    tx.execute(move_money, &[&"ada", &-30_i64]).await?;
    tx.execute(move_money, &[&"bob", &30_i64]).await?;
    tx.commit().await?;
    let bob: i64 = client
        .query_one("SELECT cents FROM accounts WHERE name = 'bob'", &[])
        .await?
        .get(0);
    assert_eq!(bob, 30);
    Ok(())
}
```

| | psql | sqlx | tokio-postgres |
|---|---|---|---|
| **Same** | `BEGIN` … `COMMIT` on one session; the prompt shows `*` inside a transaction | A transaction object; rollback when it is dropped uncommitted | A transaction object; rollback when it is dropped uncommitted |
| **Different** | Nothing stops you from forgetting `COMMIT` | `pool.begin()` takes a connection out of the pool for the transaction's lifetime | `transaction()` borrows a client mutably: the compiler stops other use of that connection, and a shared (`Arc`) client cannot start one |

---

## In stockroom

stockroom's retry helper is §8.6's, with stockroom's error type, deadpool's connections, a
proper random backoff, and an explicit rollback so that a failing rollback is at least logged:

~~~rust
// stockroom/src/db/retry.rs (excerpt)
pub async fn in_transaction<T>(
    client: &mut Object,
    isolation: IsolationLevel,
    mut work: impl AsyncFnMut(&Transaction<'_>) -> Result<T, AppError>,
) -> Result<T, AppError> {
    let mut attempt = 1;
    loop {
        let tx = client
            .build_transaction()
            .isolation_level(isolation)
            .start()
            .await?;
        let outcome = match work(&tx).await {
            Ok(value) => tx.commit().await.map(|()| value).map_err(AppError::from),
            Err(error) => {
                // Explicitly, rather than on drop, so the next attempt starts from a clean
                // session and a failed rollback is at least logged.
                if let Err(rollback) = tx.rollback().await {
                    tracing::debug!(%rollback, "rollback failed");
                }
                Err(error)
            }
        };
        match outcome {
            Err(error) if error.is_retryable() && attempt < MAX_ATTEMPTS => {
                tracing::debug!(attempt, %error, "transaction failed under concurrency; retrying");
                tokio::time::sleep(backoff(attempt)).await;
                attempt += 1;
            }
            other => return other,
        }
    }
}
~~~

`is_retryable` is true for 40001, 40P01 and 55P03, which Chapter 6's `From` impl turned into
`AppError::Busy`. A handler hands it an `async move` closure that owns the request it extracted:

~~~rust
// stockroom/src/http/orders.rs (excerpt)
pub async fn place(
    Db(mut db): Db,
    AppJson(order): AppJson<NewOrder>,
) -> Result<impl IntoResponse, AppError> {
    // `async move`: the closure owns the request, and each attempt borrows it from there.
    let order = db::in_transaction(&mut db, IsolationLevel::ReadCommitted, async move |tx| {
        orders::place(tx, &order).await
    })
    .await?;
    Ok((StatusCode::CREATED, etag(order.version), Json(order)))
}
~~~

Placing an order uses the guarded update from §8.9, one per order line, at READ COMMITTED, so
it rarely needs the retry. It only does when two orders deadlock, which the code prevents by
locking stock rows in product-id order (Chapter 7 showed why that order is reliable).
Transfers between stores are different: they read a stock level, then change two rows, and the
read must still be true at the end, so they run at SERIALIZABLE. Their first statement locks
both rows, in a fixed order, because the first version of this code deadlocked:

~~~rust
// stockroom/src/repo/stock.rs (excerpt)
// Lock both rows first, always in the same order. Otherwise a transfer A→B and one B→A
// each lock one row and wait for the other's: a deadlock, which PostgreSQL only notices
// after `deadlock_timeout` (a second). In this order the second transaction just waits,
// and when the first commits, SERIALIZABLE fails it at once ("could not serialize access
// due to concurrent update") for `in_transaction` to retry.
tx.execute(
    "SELECT FROM stock WHERE product_id = $1 AND store_id IN ($2, $3) ORDER BY store_id FOR UPDATE",
    &[&product_id, &from_id, &to_id],
)
.await?;
~~~

The test suite proves all of it with concurrency: [tests/concurrency.rs](../stockroom/tests/concurrency.rs)
places fifty orders for twenty chairs at once (exactly twenty succeed), runs forty orders for
two products listed in either order (no deadlock surfaces), and forty transfers in both
directions (no unit is lost).

## Summary

- `transaction()` sends `BEGIN`; `commit()` commits; everything else rolls back, including a
  drop (which cannot report errors) and an early `?`.
- `transaction()` borrows the client mutably: nothing else can use the connection meanwhile,
  and a shared `Arc<Client>` cannot start a transaction. Concurrent transactions need
  connections of their own.
- `build_transaction()` sets the isolation level, `read_only` and `deferrable`. READ COMMITTED
  sees others' commits between statements; REPEATABLE READ keeps one snapshot; SERIALIZABLE
  fails anything not equivalent to a serial order.
- Savepoints (`savepoint`, nested `transaction`) contain failures within a transaction.
- `GenericClient` lets one function run on a `Client` or a `Transaction`: sealed, boxed `Send`
  futures, a generic bound rather than `dyn`.
- 40001 and 40P01 mean "run the whole transaction again". A retry helper takes an async closure;
  the closure must own what it uses, and its body must be safe to repeat.
- `FOR UPDATE`, `NOWAIT`, `SKIP LOCKED`, `lock_timeout` and advisory locks control waiting.
- Optimistic concurrency: a version column, and zero rows updated means someone else won.
- To sell the last chair once: a guarded `UPDATE … WHERE` first, `FOR UPDATE` or SERIALIZABLE
  with retries when the rule does not fit one statement, and a `CHECK` constraint underneath.

## Exercises

1. **Write skew at REPEATABLE READ.** Run §8.6's doctors example at REPEATABLE READ instead of
   SERIALIZABLE. What happens, and why does snapshot isolation not catch it?
   *Hint:* each transaction checks a snapshot taken at its start, and neither writes a row that
   the other one wrote.
2. **A transfer without SERIALIZABLE.** Rewrite stockroom's transfer logic (read the source
   stock, check it, update two rows) so that it is correct at READ COMMITTED.
   *Hint:* guarded updates, and `RETURNING`.
3. **A deadlock on purpose.** With two connections, make two transactions update rows 1 and 2
   in opposite orders, and observe 40P01. How long did it take, and which setting decides
   that?
   *Hint:* `SHOW deadlock_timeout`.
4. **Claim a job.** Using `FOR UPDATE SKIP LOCKED`, write `claim_next_job(tx)` that returns the
   oldest unclaimed job or `None`, and run three workers concurrently on a table of ten jobs.
   Check that every job is processed exactly once.
   *Hint:* `SELECT … ORDER BY id LIMIT 1 FOR UPDATE SKIP LOCKED`, and mark the job done in the
   same transaction.
5. **Where does the email go?** A handler places an order and sends a confirmation email,
   inside the retried transaction body. What can go wrong, and how does stockroom avoid it?
   *Hint:* the body of a retried transaction may run more than once, and a rollback cannot
   unsend an email.

### Solutions

**1.** Both commits succeed and nobody is on call. REPEATABLE READ gives each transaction a
consistent snapshot and fails a transaction that *updates a row someone else changed*, but
here the two transactions update *different* rows; each read a row the other changed, and
only SERIALIZABLE tracks those read/write dependencies.

**2.** Do the decrement as a guarded update and the increment unconditionally, in one
transaction:

~~~sql
UPDATE stock SET on_hand = on_hand - $3
WHERE store_id = $1 AND product_id = $2 AND available >= $3
RETURNING on_hand;   -- no row: not enough stock; roll back
INSERT INTO stock (store_id, product_id, on_hand) VALUES ($4, $2, $3)
ON CONFLICT (store_id, product_id) DO UPDATE SET on_hand = stock.on_hand + EXCLUDED.on_hand;
~~~

The guard makes the check and the change atomic, and lock order still matters: two opposite
transfers can deadlock (40P01), so either lock both rows first in a fixed order, or retry.

**3.** It takes about one second: the server only looks for deadlocks after a transaction has
waited for `deadlock_timeout` (default `1s`), and then fails one of them with 40P01. That
second is why stockroom's first version of the transfer was slow under load, before it locked
rows in a fixed order.

**5.** The body can run several times: a serialization failure or deadlock means one email per
attempt, and a failed commit after a sent email means an email for an order that does not
exist. stockroom never sends anything from a transaction. The order's trigger writes a row to
the outbox *in the same transaction*, so the job exists if and only if the order committed,
and a worker sends it afterwards (Chapter 16).

---

Previous: [Chapter 7 — One connection, many tasks: pipelining](07-pipelining.md) ·
Next: [Chapter 9 — Streaming rows and portals](09-streaming.md)

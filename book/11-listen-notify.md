# Chapter 11 — LISTEN/NOTIFY and notices

> **Async PostgreSQL in Rust** · Previous: [Chapter 10 — Bulk data: COPY](10-copy.md) ·
> Next: [Chapter 12 — Production connections: timeouts, cancellation, health and TLS](12-production-connections.md)

Everything so far has been request and response: the client asks, the server answers. But the
server can also speak first. A session that has run `LISTEN orders` receives a message whenever
any session runs `NOTIFY orders`, with no query in flight and no polling. That is enough to
build live dashboards, cache invalidation and job queues that wake up the moment there is work,
with the database as the only moving part.

The server also sends *notices*, the messages that `RAISE NOTICE` and many commands produce
along the way. Both arrive unprompted, and neither reaches your code unless you drive the
`Connection` yourself. This chapter shows how, then builds the parts of a reliable listener:
a dedicated connection, a trigger that announces changes, a catch-up query for whatever was
missed, and fan-out to many subscribers.

---

## 11.1 What `NOTIFY` promises

> **SQL lens:** [`NOTIFY`](https://www.postgresql.org/docs/18/sql-notify.html) is
> transactional. A notification is queued by the statement and delivered when the transaction
> commits, and only then; after a rollback it is never delivered. Within one transaction,
> identical notifications (same channel, same payload) are delivered once. The payload is text,
> and "in the default configuration it must be shorter than 8000 bytes". Nothing is stored:
> sessions that are not listening at the moment of the commit never hear of it.

Here is each of those rules, observed from a listening session. `connect_with_messages`, a new
playground helper that §11.2 explains, gives a client plus a receiver for what the server sends
unprompted:

```rust
use std::time::Duration;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_postgres::AsyncMessage;

/// The payloads of the notifications that arrive within 200 ms.
async fn payloads(messages: &mut UnboundedReceiver<AsyncMessage>) -> Vec<String> {
    let mut seen = Vec::new();
    while let Ok(Some(message)) =
        tokio::time::timeout(Duration::from_millis(200), messages.recv()).await
    {
        if let AsyncMessage::Notification(n) = message {
            seen.push(n.payload().to_owned());
        }
    }
    seen
}

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let (listener, mut messages) = playground::connect_with_messages().await?;
    let mut notifier = playground::connect().await?;
    listener.batch_execute("LISTEN stock").await?;

    // Outside a transaction: delivered when the statement's implicit transaction commits.
    notifier.batch_execute("NOTIFY stock, 'restocked'").await?;
    assert_eq!(payloads(&mut messages).await, ["restocked"]);

    // Inside one: nothing until the commit...
    let tx = notifier.transaction().await?;
    tx.execute("SELECT pg_notify('stock', 'counted')", &[])
        .await?;
    assert!(payloads(&mut messages).await.is_empty());
    tx.commit().await?;
    assert_eq!(payloads(&mut messages).await, ["counted"]);

    // ...and nothing at all after a rollback.
    let tx = notifier.transaction().await?;
    tx.execute("SELECT pg_notify('stock', 'never')", &[])
        .await?;
    tx.rollback().await?;
    assert!(payloads(&mut messages).await.is_empty());

    // Identical notifications in one transaction are delivered once, in first-sent order.
    notifier
        .batch_execute("BEGIN; NOTIFY stock, 'a'; NOTIFY stock, 'b'; NOTIFY stock, 'a'; COMMIT")
        .await?;
    assert_eq!(payloads(&mut messages).await, ["a", "b"]);

    // 8,000 bytes is one too many.
    let err = notifier
        .execute("SELECT pg_notify('stock', $1)", &[&"x".repeat(8000)])
        .await
        .unwrap_err();
    println!("{}", err.as_db_error().expect("a server error").message());
    Ok(())
}
```

```text
payload string too long
```

Two ways to send a notification appear in that example. The `NOTIFY channel, 'payload'`
statement takes literals only: `NOTIFY stock, $1` is a syntax error, so a payload computed by
your program goes through the function `pg_notify(channel, payload)`, which takes ordinary
parameters. Use `pg_notify` from code.

The "only if committed" rule is what makes `NOTIFY` safe to put inside the transaction that
changes the data: listeners never hear about a change that did not happen. The "nothing is
stored" rule is what §11.6 is about.

## 11.2 Driving the `Connection` yourself

Chapter 1 spawned the `Connection` and awaited it. That runs all of its IO, and in 0.7.18 it
does this with anything the server sends unprompted
([`connection.rs`](https://docs.rs/tokio-postgres/0.7.18/src/tokio_postgres/connection.rs.html)):

~~~rust
// tokio-postgres 0.7.18, the Future implementation of Connection
fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
    while let Some(message) = ready!(self.poll_message(cx)?) {
        if let AsyncMessage::Notice(notice) = message {
            info!("{}: {}", notice.severity(), notice.message());
        }
    }
    Poll::Ready(Ok(()))
}
~~~

Notices go to the `log` crate at `info` level, and notifications are dropped on the floor.
The method it calls,
[`Connection::poll_message`](https://docs.rs/tokio-postgres/0.7.18/tokio_postgres/struct.Connection.html#method.poll_message),
is public: it does all the same work (sending requests, routing responses) and returns each
unprompted message as `Some(Ok(AsyncMessage))`. `None` means the connection closed cleanly and
`Some(Err(_))` that it failed; after either, it must not be polled again.

So the recipe is: spawn a task that calls `poll_message` in a loop and forwards each message to
whoever wants it. Add this to the playground's `src/lib.rs`, below `connect`, whose imports
it shares:

<!-- mirror: playground/src/lib.rs#connect_with_messages -->
~~~rust
use tokio::sync::mpsc;
use tokio_postgres::AsyncMessage;

/// Like `connect`, but drives the connection with `poll_message`, and forwards what the
/// server sends unprompted (notifications and notices) to the returned receiver instead of
/// dropping it. The channel is unbounded, which suits examples: nothing is lost, and memory is
/// the limit if nobody reads.
pub async fn connect_with_messages()
-> Result<(Client, mpsc::UnboundedReceiver<AsyncMessage>), Error> {
    let url = std::env::var("DATABASE_URL")
        .expect("set DATABASE_URL, e.g. postgres://postgres:secret@localhost:5432/tpbook");
    let (client, mut connection) = tokio_postgres::connect(&url, NoTls).await?;
    let (forward, messages) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            match std::future::poll_fn(|cx| connection.poll_message(cx)).await {
                // A closed receiver only means nobody is interested any more; the connection
                // must still be driven for the client's queries.
                Some(Ok(message)) => {
                    let _ = forward.send(message);
                }
                Some(Err(e)) => {
                    eprintln!("connection error: {e}");
                    break;
                }
                None => break,
            }
        }
    });
    Ok((client, messages))
}
~~~

`std::future::poll_fn` turns a `poll_*` method into a future of its next result: all a loop
needs. The crate's own documentation uses the stream version, `futures_util::stream::poll_fn`,
with `StreamExt::forward`; stockroom and pglisten use that one with `next()`. The three are
equivalent.

The loop must keep going for as long as the client is used, because it *is* the connection:
the client's requests are written and their responses read by these `poll_message` calls.
That is why a full channel must not block it.

> **Pitfall:** forwarding through a *bounded* channel with `send().await` gives backpressure,
> and stockroom does it (capacity 1,024), but it ties the connection to the consumer's pace. If
> the consumer falls behind and the channel fills, the driver waits, and while it waits nobody
> polls the connection: the client's own queries stall too, and the server's notification queue
> grows. Keep the consumer of a listener connection fast (or use an unbounded channel and watch
> its length), and use the listening connection for nothing else (§11.5).

## 11.3 `AsyncMessage`: notifications and notices

[`AsyncMessage`](https://docs.rs/tokio-postgres/0.7.18/tokio_postgres/enum.AsyncMessage.html)
has two variants in 0.7.18, and is `#[non_exhaustive]`, so a `match` needs a `_` arm:

- `Notification(Notification)`, with `channel()`, `payload()`, and `process_id()`: the backend
  PID of the session that sent it.
- `Notice(DbError)`: the same type as a server error (Chapter 6), with `severity()`, `code()`
  and `message()`, but not an error. `RAISE NOTICE`, `RAISE WARNING` and many commands
  (`DROP TABLE IF EXISTS` on a missing table, for one) send them.

```rust
use std::time::Duration;
use tokio_postgres::AsyncMessage;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let (client, mut messages) = playground::connect_with_messages().await?;
    let pid: i32 = client
        .query_one("SELECT pg_backend_pid()", &[])
        .await?
        .get(0);
    client
        .batch_execute(r#"LISTEN "Orders"; LISTEN orders"#)
        .await?;
    client
        .batch_execute(
            r#"NOTIFY Orders, 'unquoted';
               SELECT pg_notify('Orders', 'pg_notify keeps the case');
               DO $$ BEGIN
                   RAISE NOTICE 'checked % rows', 42;
                   RAISE WARNING 'stock is low';
                   RAISE DEBUG 'not sent: below client_min_messages';
               END $$;"#,
        )
        .await?;

    let mut seen = Vec::new();
    while let Ok(Some(message)) =
        tokio::time::timeout(Duration::from_millis(200), messages.recv()).await
    {
        match message {
            AsyncMessage::Notification(n) => {
                assert_eq!(n.process_id(), pid); // a session hears its own notifications
                seen.push(format!(
                    "notification on {:?}: {}",
                    n.channel(),
                    n.payload()
                ));
            }
            AsyncMessage::Notice(notice) => {
                seen.push(format!(
                    "{} ({}): {}",
                    notice.severity(),
                    notice.code().code(),
                    notice.message()
                ));
            }
            _ => {} // the enum is #[non_exhaustive]
        }
    }
    for line in &seen {
        println!("{line}");
    }
    assert_eq!(seen.len(), 4);
    Ok(())
}
```

```text
NOTICE (00000): checked 42 rows
WARNING (01000): stock is low
notification on "orders": unquoted
notification on "Orders": pg_notify keeps the case
```

Three details are in that output:

- **Notices come during execution, notifications at the commit.** The batch is one implicit
  transaction, so both notifications arrived after both notices, although they were sent
  first.
- **Channel names are identifiers.** Unquoted, `NOTIFY Orders` and `LISTEN Orders` fold to
  `orders`; `pg_notify('Orders', …)` takes a string and keeps the case, so only a quoted
  `LISTEN "Orders"` hears it. When channel names come from configuration, quote them (pglisten's
  `quote_ident`, at the end of this chapter), and they then match `pg_notify` exactly.
- **`client_min_messages`** (default `notice`) decides which notices the server sends at all:
  the `DEBUG` line never left the server.

## 11.4 Triggers that announce changes

Calling `pg_notify` from application code works, but every code path that changes the data
must remember to do it. A trigger cannot forget. It runs in the transaction of the change, so
the notification shares its fate: delivered if the change commits, never if it rolls back.

```rust
use serde::Deserialize;
use std::time::Duration;
use tokio_postgres::AsyncMessage;

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct StockEvent {
    product: String,
    on_hand: i32,
    previous: Option<i32>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (client, mut messages) = playground::connect_with_messages().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE stock (product text PRIMARY KEY, on_hand int NOT NULL);

             CREATE FUNCTION pg_temp.announce_stock() RETURNS trigger
             LANGUAGE plpgsql AS $$
             BEGIN
                 PERFORM pg_notify('stock_events', json_build_object(
                     'product',  NEW.product,
                     'onHand',   NEW.on_hand,
                     'previous', CASE WHEN TG_OP = 'UPDATE' THEN OLD.on_hand END
                 )::text);
                 RETURN NULL;
             END $$;

             CREATE TRIGGER stock_announce
                 AFTER INSERT OR UPDATE OF on_hand ON stock
                 FOR EACH ROW EXECUTE FUNCTION pg_temp.announce_stock();

             LISTEN stock_events;",
        )
        .await?;

    client
        .batch_execute(
            "INSERT INTO stock VALUES ('chair', 10);
             UPDATE stock SET on_hand = on_hand - 3 WHERE product = 'chair';",
        )
        .await?;

    let mut events = Vec::new();
    while let Ok(Some(message)) =
        tokio::time::timeout(Duration::from_millis(200), messages.recv()).await
    {
        if let AsyncMessage::Notification(n) = message {
            events.push(serde_json::from_str::<StockEvent>(n.payload())?);
        }
    }
    assert_eq!(
        events,
        [
            StockEvent {
                product: "chair".into(),
                on_hand: 10,
                previous: None
            },
            StockEvent {
                product: "chair".into(),
                on_hand: 7,
                previous: Some(10)
            },
        ]
    );
    Ok(())
}
```

(The function lives in `pg_temp`, like the table, so that the example leaves nothing behind;
in a real schema it is an ordinary function created by a migration.)

Keep payloads small. The 8,000-byte limit is one reason; another is that a notification is a
*hint*, not a record. Send the identifiers and the little that a listener needs to decide
whether to care, and let it read the current state if it does. A payload that carries the
whole row tempts listeners to trust it, and §11.6 shows why they should not trust it alone.

## 11.5 A connection of its own

The listening session must be a connection that does nothing else, and in particular not one
from the pool:

- **A pool awaits its connections.** deadpool-postgres spawns each `Connection` as a future
  (`connection.await`, in its `lib.rs`), so notifications that arrive on a pooled connection
  are dropped, as §11.2 showed.
- **`LISTEN` belongs to the session.** A pooled connection is someone else's session a moment
  later. deadpool's `RecyclingMethod::Clean` even runs `UNLISTEN *` when the connection comes
  back (Chapter 13).
- **A listener in a transaction hears nothing until the transaction ends.** The server holds
  notifications back from a session that is inside a transaction, so a connection that also
  does work delivers them late:

```rust
use std::time::Duration;
use tokio_postgres::AsyncMessage;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let (mut listener, mut messages) = playground::connect_with_messages().await?;
    let notifier = playground::connect().await?;
    listener.batch_execute("LISTEN jobs").await?;

    let tx = listener.transaction().await?;
    tx.execute("SELECT 1", &[]).await?;
    notifier
        .execute("SELECT pg_notify('jobs', 'work')", &[])
        .await?;
    let during = tokio::time::timeout(Duration::from_millis(300), messages.recv()).await;
    assert!(
        during.is_err(),
        "nothing arrives while the listener is in a transaction"
    );

    tx.commit().await?;
    let after = tokio::time::timeout(Duration::from_millis(300), messages.recv()).await;
    assert!(matches!(after, Ok(Some(AsyncMessage::Notification(_)))));
    Ok(())
}
```

A listener connection is cheap: one per process, however many channels and subscribers. Open
it from the same `Config` as the pool, drive it with `poll_message`, `LISTEN` on every channel
the process needs, and distribute what arrives inside the process (§11.7).

## 11.6 Reliability: listen, catch up, deduplicate

Notifications are not stored. A listener that is reconnecting, restarting, or not yet started
misses whatever was committed in the meantime, and nothing tells it so. A reliable consumer
therefore treats notifications as a fast path over a slow path that is always correct: a query
that finds everything it has not handled yet.

The order of the steps matters:

1. `LISTEN` first.
2. Then run the catch-up query, which reads everything committed so far.
3. Then handle notifications as they arrive, and ignore the ones the catch-up already
   covered.

A change committed between steps 1 and 2 is seen twice, once by each path, which
deduplication handles. Run the steps the other way round, and a change committed between the
query and the `LISTEN` is seen by neither. This example inserts on the listening session
itself, which makes the order deterministic:

```rust
use std::collections::BTreeSet;
use std::time::Duration;
use tokio_postgres::AsyncMessage;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let (client, mut messages) = playground::connect_with_messages().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE event (id bigint GENERATED ALWAYS AS IDENTITY, body text);
             CREATE FUNCTION pg_temp.announce_event() RETURNS trigger
             LANGUAGE plpgsql AS $$
             BEGIN
                 PERFORM pg_notify('events', NEW.id::text);
                 RETURN NULL;
             END $$;
             CREATE TRIGGER event_announce AFTER INSERT ON event
                 FOR EACH ROW EXECUTE FUNCTION pg_temp.announce_event();",
        )
        .await?;
    let insert = client
        .prepare("INSERT INTO event (body) VALUES ($1)")
        .await?;

    client
        .execute(&insert, &[&"before anyone listened"])
        .await?; // id 1
    // 1. LISTEN first.
    client.batch_execute("LISTEN events").await?;
    client
        .execute(&insert, &[&"between LISTEN and catch-up"])
        .await?; // id 2
    // 2. Catch up on everything committed so far.
    let mut handled = BTreeSet::new();
    for row in client
        .query("SELECT id FROM event ORDER BY id", &[])
        .await?
    {
        handled.insert(row.get::<_, i64>(0));
    }
    client.execute(&insert, &[&"after catch-up"]).await?; // id 3

    // 3. Notifications, deduplicated against what was handled.
    let mut duplicates = 0;
    while let Ok(Some(message)) =
        tokio::time::timeout(Duration::from_millis(200), messages.recv()).await
    {
        if let AsyncMessage::Notification(n) = message {
            let id: i64 = n.payload().parse().expect("an id");
            if !handled.insert(id) {
                duplicates += 1;
            }
        }
    }
    assert_eq!(handled.into_iter().collect::<Vec<_>>(), [1, 2, 3]);
    assert_eq!(duplicates, 1); // event 2, seen by both paths
    Ok(())
}
```

In a service, "everything committed so far" is rarely the whole table: it is the rows newer
than a checkpoint the consumer stored (an id or a timestamp), or the rows of an *outbox* table
not yet marked done. And the steps repeat after every reconnect, because a new connection is a
new session that has missed everything since the old one died. stockroom publishes a `Resync`
event to its subscribers each time its listener reconnects, which tells them to re-read
instead of trusting the stream.

One server-side limit completes the picture. Notifications wait in a queue ("8GB in a
standard installation") until every listening session has read them, and "no cleanup can take
place if a session executes `LISTEN` and then enters a transaction for a very long time"
([`NOTIFY`](https://www.postgresql.org/docs/18/sql-notify.html)). If the queue fills,
transactions that `NOTIFY` fail at commit: one forgotten listener in a long transaction can
break every writer. `SELECT pg_notification_queue_usage()` reports how full it is, as a
fraction; watch it in production.

## 11.7 Fan-out with `broadcast`

One listener connection per process, but many consumers inside it: one per open Server-Sent
Events stream, say (Chapter 16). `tokio::sync::broadcast` is made for this. Every subscriber
gets every message, and the channel holds the last `capacity` messages. A subscriber that falls
further behind than that does not block anyone: its next `recv` returns
`RecvError::Lagged(n)`, saying how many it missed, and continues with the oldest message still
held. That is exactly the "you missed something, re-read" signal of §11.6:

```rust
use tokio::sync::broadcast;
use tokio_postgres::AsyncMessage;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let (client, mut messages) = playground::connect_with_messages().await?;
    client.batch_execute("LISTEN prices").await?;
    client
        .batch_execute(
            "BEGIN;
             SELECT pg_notify('prices', i::text) FROM generate_series(1, 10) i;
             COMMIT;",
        )
        .await?;

    // The listener task publishes; capacity 4 keeps the last four events.
    let (publish, mut slow_subscriber) = broadcast::channel::<String>(4);
    let mut published = 0;
    while published < 10 {
        if let Some(AsyncMessage::Notification(n)) = messages.recv().await {
            let _ = publish.send(n.payload().to_owned()); // Err only means no subscribers
            published += 1;
        }
    }

    // This subscriber did not read while the ten were published.
    let mut received = Vec::new();
    let mut lagged = 0;
    loop {
        match slow_subscriber.try_recv() {
            Ok(payload) => received.push(payload),
            Err(broadcast::error::TryRecvError::Lagged(n)) => lagged += n, // re-read here
            Err(_) => break,                                               // empty
        }
    }
    assert_eq!(lagged, 6);
    assert_eq!(received, ["7", "8", "9", "10"]);
    Ok(())
}
```

`recv().await` reports a lag the same way, as `Err(RecvError::Lagged(n))`. Pick the capacity
for the slowest subscriber you are willing to keep up to date without a resync, and make the resync
cheap: a subscriber that lagged re-reads the current state, which is the same query as the
catch-up of §11.6.

### Side by side — *subscribing to events*

The task: receive the notifications sent on `order_events`.

```sql
-- psql prints notifications after the next command it runs, or while it waits for input.
tpbook=# LISTEN order_events;
LISTEN
tpbook=# NOTIFY order_events, '{"orderId": 42, "status": "paid"}';
NOTIFY
Asynchronous notification "order_events" with payload "{"orderId": 42, "status": "paid"}" received from server process with PID 28381.
```

~~~rust
// sqlx (illustration)
let mut listener = sqlx::postgres::PgListener::connect_with(&pool).await?;
listener.listen("order_events").await?;
loop {
    // Reconnects by itself after a lost connection; what was sent meanwhile is lost.
    let notification = listener.recv().await?;
    println!("{}: {}", notification.channel(), notification.payload());
}
~~~

```rust
use tokio_postgres::AsyncMessage;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let (client, mut messages) = playground::connect_with_messages().await?;
    client.batch_execute("LISTEN order_events").await?;
    client
        .execute(
            "SELECT pg_notify('order_events', $1)",
            &[&r#"{"orderId": 42, "status": "paid"}"#],
        )
        .await?;
    loop {
        match messages.recv().await {
            Some(AsyncMessage::Notification(n)) => {
                println!("{}: {}", n.channel(), n.payload());
                assert_eq!(n.channel(), "order_events");
                break;
            }
            Some(_) => {}
            None => panic!("the connection closed"),
        }
    }
    Ok(())
}
```

| | psql | sqlx | tokio-postgres |
|---|---|---|---|
| **Same** | `LISTEN` on a session; notifications arrive after commit, with channel, payload and sender PID | `LISTEN` on a session; notifications arrive after commit, with channel, payload and sender PID | `LISTEN` on a session; notifications arrive after commit, with channel, payload and sender PID |
| **Different** | Printed between commands | A dedicated `PgListener` connection that reconnects and re-listens by itself, losing what was sent meanwhile | You drive the `Connection` with `poll_message`; reconnecting, re-listening and catching up are yours to write |

---

## In stockroom

stockroom announces orders with a trigger, in the migration that creates the outbox:

~~~sql
-- stockroom/migrations/0002_order_events.sql (excerpt)
CREATE FUNCTION announce_order_event() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    event jsonb := jsonb_build_object(
        'orderId',  NEW.id,
        'storeId',  NEW.store_id,
        'status',   NEW.status,
        'previous', CASE WHEN TG_OP = 'UPDATE' THEN OLD.status END,
        'version',  NEW.version
    );
BEGIN
    PERFORM pg_notify('order_events', event::text);
    INSERT INTO outbox (topic, payload) VALUES ('order.' || NEW.status, event);
    PERFORM pg_notify('outbox', '');
    RETURN NULL;
END
$$;
~~~

[stockroom/migrations/0002_order_events.sql](../stockroom/migrations/0002_order_events.sql)

Each change is announced twice. `order_events` carries a small JSON event for live screens,
which may miss some and resync. The outbox row is the durable record for the background worker
(Chapter 16), and the payload-less `pg_notify('outbox', '')` only wakes it: since identical
notifications in one transaction are delivered once, a transaction that places a hundred orders
wakes the worker once.

`notify::listener` holds the dedicated connection. It drives it with `poll_message` through a
bounded channel, listens, publishes `Resync`, and then dispatches; `run` wraps it in a
reconnect loop with exponential backoff:

~~~rust
// stockroom/src/notify/listener.rs (excerpt)
let (client, mut connection) = config.connect(tls.clone()).await?;
hub.set_server_info(server_info(&connection));

let (forward, mut messages) = mpsc::channel(1024);
let driver = tokio::spawn(async move {
    let mut incoming = stream::poll_fn(move |cx| connection.poll_message(cx));
    while let Some(message) = incoming.next().await {
        if forward.send(message?).await.is_err() {
            break; // the listener is gone
        }
    }
    Ok::<_, tokio_postgres::Error>(())
});

client
    .batch_execute("LISTEN order_events; LISTEN outbox")
    .await?;
tracing::info!("listening for order events");
// Anything sent while nobody was listening is lost for good. Tell subscribers to
// re-read, and the worker to look for jobs it was not woken for.
hub.publish(HubEvent::Resync);
hub.wake_outbox();
// … then a select! loop over shutdown and `messages`, dispatching each notification
~~~

[stockroom/src/notify/listener.rs](../stockroom/src/notify/listener.rs) ·
[stockroom/src/notify/mod.rs](../stockroom/src/notify/mod.rs)

The `Hub` in `notify/mod.rs` is §11.7's pattern: a `broadcast` channel of `HubEvent`s, where
`HubEvent::Resync` is the "re-read" signal, plus a `tokio::sync::Notify` for the worker. A
`Notify` keeps one pending wake-up even if the worker is busy when it arrives, so a burst of
outbox notifications costs the worker one extra look, never a missed job.

The listener also records what the server said about itself when it connected, through
`Connection::parameter` (Chapter 1): `GET /admin/sessions` shows the server version and time
zone that the listener saw.

**pglisten** (`apps/pglisten`) is the same design as a command-line tool, with the catch-up
query of §11.6 as an option and a `check_connection` ping so that a dead connection is noticed
even when nothing is notified (Chapter 12):

```bash
cargo run -p pglisten -- order_events outbox
cargo run -p pglisten -- --catch-up "SELECT id, topic FROM outbox WHERE done_at IS NULL" outbox
```

Channel names come from the command line, and identifiers cannot be parameters (`LISTEN $1` is
a syntax error), so this is one of the few places where text from outside goes into SQL. It
goes through `quote_ident`:

~~~rust
// apps/pglisten/src/lib.rs (excerpt)
pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
~~~

[apps/pglisten/src/lib.rs](../apps/pglisten/src/lib.rs)

## Summary

- `NOTIFY` is delivered at commit, never after a rollback, once per identical
  channel-and-payload within a transaction, with a payload shorter than 8,000 bytes. Nothing is
  stored. Use `pg_notify(channel, payload)` from code: it takes parameters.
- Awaiting a `Connection` drops notifications and logs notices. Drive it with
  `poll_message` in a loop and forward what it returns; never stop polling while the client is
  in use.
- `AsyncMessage` is `Notification` (channel, payload, sender PID) or `Notice` (a `DbError` that
  is not an error), and is non-exhaustive. Notices arrive during execution, notifications at
  commit. Unquoted channel names fold to lower case.
- Announce changes from a trigger, in the transaction of the change, with a small payload of
  identifiers.
- Listen on a dedicated connection, never a pooled one, and keep it out of transactions.
- Reliability: `LISTEN`, then catch up with a query, then deduplicate notifications against
  what the query found. Repeat after every reconnect.
- Fan out inside the process with `broadcast`; treat `Lagged` as "resync".

## Exercises

1. **How many wake-ups?** A transaction inserts 1,000 orders into stockroom. How many
   notifications does a listener on `order_events` receive, and how many on `outbox`? Check
   your answer against stockroom's trigger.
   *Hint:* §11.1's rule about identical notifications.
2. **The gap.** Use pglisten with a catch-up query, and kill its connection from psql with
   `SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE application_name = …`. While
   it reconnects, insert an outbox row. Which path reports the row, and what would have
   happened without the catch-up query?
   *Hint:* pglisten backs off for 100 ms before its first reconnect; `\watch` can insert rows in
   a loop.
3. **A stalled driver.** Change a copy of `connect_with_messages` to forward through
   `mpsc::channel(1)` with `send().await`, and never read the receiver. Run a `DO` block that
   raises two notices, with a timeout. What happens, and why?
   *Hint:* who reads the statement's own answer from the socket?
4. **A safe payload.** Rewrite §11.4's trigger so that it sends the JSON when it fits in 7,999
   bytes, and only the product key otherwise.
   *Hint:* `octet_length(text)`.

### Solutions

**1.** 1,000 on `order_events`, because each payload carries a different order id, and one on
`outbox`, whose payload is always empty: a thousand identical notifications in one
transaction are delivered once.

**2.** The catch-up query reports it, after the reconnect. The notification was sent at
commit to the sessions listening at that moment, and pglisten's new session did not exist yet,
so without the catch-up query the row would wait until the next notification on `outbox`
happened to wake someone.

**3.** The statement times out, although the server finished it at once. The driver task
receives the first notice and sends it into the channel, which fills it; the second
`send().await` waits for room that never comes. While it waits, nothing calls `poll_message`,
and that is the call that reads responses from the socket and writes new requests to it. The
statement's *CommandComplete* stays unread, and every later request stays unsent:

```rust
use std::time::Duration;
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let (client, mut connection) = tokio_postgres::connect(&url, tokio_postgres::NoTls).await?;
    let (forward, _never_read) = mpsc::channel(1);
    tokio::spawn(async move {
        while let Some(Ok(message)) = std::future::poll_fn(|cx| connection.poll_message(cx)).await {
            if forward.send(message).await.is_err() {
                break;
            }
        }
    });

    // The first notice fills the channel; the second leaves the driver waiting for room.
    let batch = client.batch_execute("DO $$ BEGIN RAISE NOTICE 'one'; RAISE NOTICE 'two'; END $$");
    let result = tokio::time::timeout(Duration::from_millis(500), batch).await;
    assert!(
        result.is_err(),
        "the driver is stuck, so the answer is never read"
    );
    Ok(())
}
```

**4.**

```sql
CREATE FUNCTION announce_stock() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    payload text := json_build_object(
        'product', NEW.product,
        'onHand',  NEW.on_hand,
        'previous', CASE WHEN TG_OP = 'UPDATE' THEN OLD.on_hand END
    )::text;
BEGIN
    IF octet_length(payload) >= 8000 THEN
        payload := json_build_object('product', NEW.product)::text;
    END IF;
    PERFORM pg_notify('stock_events', payload);
    RETURN NULL;
END $$;
```

Listeners must then accept both shapes, which they should anyway: a notification is a hint to
re-read, not the data itself. (A product key long enough to break even this is a schema
problem: limit it with a `CHECK`.)

---

Previous: [Chapter 10 — Bulk data: COPY](10-copy.md) ·
Next: [Chapter 12 — Production connections: timeouts, cancellation, health and TLS](12-production-connections.md)

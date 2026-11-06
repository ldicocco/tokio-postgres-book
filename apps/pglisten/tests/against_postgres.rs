//! pglisten against a real server: DATABASE_URL from the environment or `.env`.

use std::time::Duration;

use pglisten::{Event, Options};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_postgres::{Client, Config, NoTls};
use tokio_util::sync::CancellationToken;

fn config(application_name: &str) -> Config {
    let _ = dotenvy::dotenv();
    let url = std::env::var("DATABASE_URL").expect("set DATABASE_URL (see .env.example)");
    let mut config: Config = url.parse().unwrap();
    config.application_name(application_name);
    config
}

async fn client() -> Client {
    let (client, connection) = config("pglisten-test-sender").connect(NoTls).await.unwrap();
    tokio::spawn(connection);
    client
}

/// Starts the listener; its events arrive on the returned channel.
fn start(name: &str, options: Options) -> (mpsc::UnboundedReceiver<Event>, CancellationToken) {
    let (tx, rx) = mpsc::unbounded_channel();
    let shutdown = CancellationToken::new();
    tokio::spawn(pglisten::run(
        config(name),
        options,
        move |e| drop(tx.send(e)),
        shutdown.clone(),
    ));
    (rx, shutdown)
}

async fn next(events: &mut mpsc::UnboundedReceiver<Event>) -> Event {
    timeout(Duration::from_secs(10), events.recv())
        .await
        .expect("an event in time")
        .unwrap()
}

#[tokio::test]
async fn notifications_on_a_quoted_channel_arrive() {
    // Upper case and a quote: only quote_ident makes LISTEN match pg_notify exactly.
    let channel = format!("Test \"{}\"", uuid::Uuid::now_v7().simple());
    let (mut events, shutdown) = start(
        "pglisten-test-1",
        Options {
            channels: vec![channel.clone()],
            ..Default::default()
        },
    );
    assert!(matches!(
        next(&mut events).await,
        Event::Listening {
            server_version: Some(_)
        }
    ));

    let sender = client().await;
    sender
        .execute("SELECT pg_notify($1, 'hello')", &[&channel])
        .await
        .unwrap();
    // Inside a transaction, a notification is delivered at commit, and only then.
    let mut sender = sender;
    let tx = sender.transaction().await.unwrap();
    tx.execute("SELECT pg_notify($1, 'rolled back')", &[&channel])
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    let tx = sender.transaction().await.unwrap();
    tx.execute("SELECT pg_notify($1, 'committed')", &[&channel])
        .await
        .unwrap();
    tx.commit().await.unwrap();

    for expected in ["hello", "committed"] {
        match next(&mut events).await {
            Event::Notification {
                channel: got,
                payload,
                ..
            } => {
                assert_eq!(
                    (got.as_str(), payload.as_str()),
                    (channel.as_str(), expected)
                )
            }
            other => panic!("expected a notification, got {other:?}"),
        }
    }
    shutdown.cancel();
}

#[tokio::test]
async fn after_losing_the_connection_it_reconnects_and_catches_up() {
    let channel = format!("t{}", uuid::Uuid::now_v7().simple());
    let name = format!("pglisten-{}", &channel[..20]);
    let options = Options {
        channels: vec![channel.clone()],
        catch_up: Some("SELECT 'caught up' AS what".into()),
        ping: None,
    };
    let (mut events, shutdown) = start(&name, options);
    assert!(matches!(next(&mut events).await, Event::Listening { .. }));
    assert!(matches!(next(&mut events).await, Event::CaughtUp { .. }));

    // The server ends the listener's session.
    let admin = client().await;
    admin
        .execute(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE application_name = $1",
            &[&name],
        )
        .await
        .unwrap();
    assert!(matches!(next(&mut events).await, Event::Lost { .. }));
    assert!(matches!(next(&mut events).await, Event::Listening { .. }));
    match next(&mut events).await {
        Event::CaughtUp { rows } => assert_eq!(rows, vec![vec![Some("caught up".to_owned())]]),
        other => panic!("expected the catch-up, got {other:?}"),
    }
    // And it is listening again.
    admin
        .execute("SELECT pg_notify($1, 'again')", &[&channel])
        .await
        .unwrap();
    assert!(
        matches!(next(&mut events).await, Event::Notification { payload, .. } if payload == "again")
    );
    shutdown.cancel();
}

#[tokio::test]
async fn notices_are_reported() {
    let channel = format!("t{}", uuid::Uuid::now_v7().simple());
    let options = Options {
        channels: vec![channel],
        // A catch-up query that makes the server send a NOTICE.
        catch_up: Some("DO $$ BEGIN RAISE NOTICE 'from the catch-up'; END $$".into()),
        ping: Some(Duration::from_millis(200)),
    };
    let (mut events, shutdown) = start("pglisten-test-3", options);
    let mut saw_notice = false;
    for _ in 0..3 {
        if let Event::Notice { message, .. } = next(&mut events).await {
            assert_eq!(message, "from the catch-up");
            saw_notice = true;
            break;
        }
    }
    assert!(saw_notice);
    shutdown.cancel();
}

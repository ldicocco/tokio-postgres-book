//! `stockroom serve`: everything wired together, and taken apart again at shutdown.

use std::time::Duration;

use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::config::{DbArgs, ServeArgs};
use crate::http::{AppState, RouterOptions};
use crate::notify::Hub;
use crate::worker::outbox::LogDelivery;
use crate::{db, http, notify, worker};

pub async fn serve(db_args: DbArgs, args: ServeArgs) -> anyhow::Result<()> {
    let tls = db::tls::make_tls(&db_args)?;
    let config = db_args.pg_config()?;

    if args.migrate {
        let mut client = db::connect(&config, &tls).await?;
        db::migrate::run(&mut client).await?;
    }

    let pool = db::pool::create(config.clone(), tls.clone(), args.pool_size)?;
    let hub = Hub::new(1024);
    let shutdown = CancellationToken::new();

    let listener = tokio::spawn(notify::listener::run(
        config,
        tls.clone(),
        hub.clone(),
        shutdown.clone(),
    ));
    let worker = tokio::spawn(worker::outbox::run(
        pool.clone(),
        hub.clone(),
        LogDelivery,
        shutdown.clone(),
    ));

    let state = AppState {
        pool: pool.clone(),
        hub,
        tls,
        shutdown: shutdown.clone(),
    };
    let options = RouterOptions {
        request_timeout: Duration::from_millis(args.request_timeout_ms),
        ..Default::default()
    };
    let app = http::router(state, options);

    let tcp = TcpListener::bind(args.bind).await?;
    tracing::info!("listening on http://{}", tcp.local_addr()?);
    tokio::spawn(cancel_on_signal(shutdown.clone()));

    // Stop accepting connections when `shutdown` fires, then wait for in-flight requests.
    axum::serve(tcp, app)
        .with_graceful_shutdown(shutdown.clone().cancelled_owned())
        .await?;

    shutdown.cancel();
    let _ = tokio::join!(listener, worker);
    pool.close();
    tracing::info!("shut down");
    Ok(())
}

/// Ctrl-C, or SIGTERM from an orchestrator, starts a graceful shutdown.
async fn cancel_on_signal(shutdown: CancellationToken) {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
    tracing::info!("shutting down");
    shutdown.cancel();
}

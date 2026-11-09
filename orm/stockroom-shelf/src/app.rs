//! `stockroom-shelf serve`: stockroom's `app::serve`, with this crate's router, and every model
//! verified against the database before the first request.

use std::time::Duration;

use anyhow::{Context, bail};
use stockroom::config::{DbArgs, ServeArgs};
use stockroom::db;
use stockroom::http::{AppState, RouterOptions};
use stockroom::notify::{self, Hub};
use stockroom::worker::{self, outbox::LogDelivery};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::{http, repo};

pub async fn serve(db_args: DbArgs, args: ServeArgs) -> anyhow::Result<()> {
    let tls = db::tls::make_tls(&db_args)?;
    let config = db_args.pg_config()?;

    if args.migrate {
        let mut client = db::connect(&config, &tls).await?;
        db::migrate::run(&mut client).await?;
    }

    let pool = db::pool::create(config.clone(), tls.clone(), args.pool_size)?;
    verify(&pool).await?;
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
    let router = http::router(state, options);

    let tcp = TcpListener::bind(args.bind).await?;
    tracing::info!("listening on http://{}", tcp.local_addr()?);
    tokio::spawn(cancel_on_signal(shutdown.clone()));

    axum::serve(tcp, router)
        .with_graceful_shutdown(shutdown.clone().cancelled_owned())
        .await?;

    shutdown.cancel();
    let _ = tokio::join!(listener, worker);
    pool.close();
    tracing::info!("shut down");
    Ok(())
}

/// Ctrl-C, or SIGTERM from an orchestrator, starts a graceful shutdown. (stockroom's, which is
/// private to its `app` module.)
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

/// Every model against the database, on a pooled connection: an error lists every mismatch.
pub async fn verify(pool: &deadpool_postgres::Pool) -> anyhow::Result<()> {
    let db = pool.get().await.context("no database connection")?;
    match repo::verify_models(&db).await {
        Ok(()) => Ok(()),
        Err(errors) => {
            let all: Vec<String> = errors.iter().map(ToString::to_string).collect();
            bail!(
                "the models do not match the database:\n  {}",
                all.join("\n  ")
            )
        }
    }
}

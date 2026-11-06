//! Settings: command-line flags with environment fallbacks (clap's `env`), turned into a
//! `tokio_postgres::Config` (Chapter 1, Chapter 12).

use std::net::SocketAddr;
use std::time::Duration;

use clap::{ArgAction, Args};
use tokio_postgres::Config;

/// How to reach PostgreSQL. Every subcommand takes these.
#[derive(Debug, Clone, Args)]
pub struct DbArgs {
    /// PostgreSQL connection string: a postgres:// URL or `key=value` pairs.
    #[arg(long, env = "STOCKROOM_DATABASE_URL", hide_env_values = true)]
    pub database_url: String,

    /// Server-side limit for any one statement, in milliseconds (0 = no limit).
    #[arg(long, env = "STOCKROOM_STATEMENT_TIMEOUT_MS", default_value_t = 5_000)]
    pub statement_timeout_ms: u64,

    /// PEM file with the certificate authority that signed the server's certificate.
    /// Without it, the operating system's trusted roots are used.
    #[cfg(feature = "tls")]
    #[arg(long, env = "STOCKROOM_DB_CA_FILE")]
    pub db_ca_file: Option<std::path::PathBuf>,
}

impl DbArgs {
    /// The settings for `database_url`, with every other field at its default.
    pub fn new(database_url: impl Into<String>) -> DbArgs {
        DbArgs {
            database_url: database_url.into(),
            statement_timeout_ms: 5_000,
            #[cfg(feature = "tls")]
            db_ca_file: None,
        }
    }

    /// The connection string, plus the settings every stockroom session should have unless
    /// the string says otherwise.
    pub fn pg_config(&self) -> Result<Config, tokio_postgres::Error> {
        let mut config: Config = self.database_url.parse()?;
        if config.get_application_name().is_none() {
            config.application_name("stockroom");
        }
        if config.get_connect_timeout().is_none() {
            config.connect_timeout(Duration::from_secs(5));
        }
        // Notice a dead peer in about a minute rather than the default two hours.
        config.keepalives_idle(Duration::from_secs(30));
        config.keepalives_interval(Duration::from_secs(10));
        config.keepalives_retries(3);

        // Guard rails on the server: runaway statements, lock waits, and transactions left
        // open by a crashed or stuck client are all ended by PostgreSQL itself.
        let mut options = config.get_options().unwrap_or_default().to_owned();
        for setting in [
            format!("statement_timeout={}", self.statement_timeout_ms),
            "lock_timeout=2000".to_owned(),
            "idle_in_transaction_session_timeout=60000".to_owned(),
        ] {
            if !options.is_empty() {
                options.push(' ');
            }
            options.push_str("-c ");
            options.push_str(&setting);
        }
        config.options(options);
        Ok(config)
    }
}

/// `stockroom serve`.
#[derive(Debug, Clone, Args)]
pub struct ServeArgs {
    /// Address to listen on.
    #[arg(long, env = "STOCKROOM_BIND", default_value = "127.0.0.1:8080")]
    pub bind: SocketAddr,

    /// Maximum number of pooled database connections.
    #[arg(long, env = "STOCKROOM_POOL_SIZE", default_value_t = 16)]
    pub pool_size: usize,

    /// Time limit for producing a response, in milliseconds. Streaming bodies are not limited.
    #[arg(long, env = "STOCKROOM_REQUEST_TIMEOUT_MS", default_value_t = 10_000)]
    pub request_timeout_ms: u64,

    /// Apply pending migrations before serving.
    #[arg(long, env = "STOCKROOM_MIGRATE", default_value_t = true, action = ArgAction::Set)]
    pub migrate: bool,
}

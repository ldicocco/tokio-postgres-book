//! Embedded migrations (Chapter 14).
//!
//! The SQL files are compiled into the binary, so the schema a build expects travels with it.
//! Running them is safe from several instances at once (an advisory lock), each migration is
//! all or nothing (DDL is transactional in PostgreSQL), and a migration that was edited after
//! it was applied is refused rather than silently ignored (a checksum).

use std::collections::HashMap;

use tokio_postgres::Client;

pub struct Migration {
    pub version: i32,
    pub name: &'static str,
    pub sql: &'static str,
}

pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "init",
        sql: include_str!("../../migrations/0001_init.sql"),
    },
    Migration {
        version: 2,
        name: "order_events",
        sql: include_str!("../../migrations/0002_order_events.sql"),
    },
];

/// The advisory lock all instances agree on: "STOCKROO" in ASCII.
const LOCK_KEY: i64 = 0x5354_4f43_4b52_4f4f;

#[derive(Debug, thiserror::Error)]
pub enum MigrateError {
    #[error(transparent)]
    Db(#[from] tokio_postgres::Error),
    #[error(
        "migration {version} ({name}) was edited after it was applied: \
         checksum {applied} in the database, {current} in this build"
    )]
    Edited {
        version: i32,
        name: String,
        applied: String,
        current: String,
    },
    #[error(
        "the database has migration {version}, which this build does not know: is the build older than the schema?"
    )]
    Unknown { version: i32 },
}

/// Applies the pending migrations of this build; returns the versions it applied.
pub async fn run(client: &mut Client) -> Result<Vec<i32>, MigrateError> {
    run_with(client, MIGRATIONS).await
}

/// [`run`] with a list of migrations of the caller's choosing (the tests use it).
pub async fn run_with(
    client: &mut Client,
    migrations: &[Migration],
) -> Result<Vec<i32>, MigrateError> {
    // Migrations may take long and wait for locks: lift this session's guard rails.
    client
        .batch_execute("SET statement_timeout = 0; SET lock_timeout = 0")
        .await?;
    // A session-level lock, held across the transactions below. A second instance waits
    // here, then finds nothing left to do.
    client
        .execute("SELECT pg_advisory_lock($1)", &[&LOCK_KEY])
        .await?;
    let result = apply(client, migrations).await;
    // Released explicitly, because the connection may live on; it would also be released
    // if the connection closed.
    client
        .execute("SELECT pg_advisory_unlock($1)", &[&LOCK_KEY])
        .await?;
    result
}

async fn apply(client: &mut Client, migrations: &[Migration]) -> Result<Vec<i32>, MigrateError> {
    // On every run but the first this logs a NOTICE ("relation … already exists, skipping"),
    // courtesy of `db::connect`.
    client
        .batch_execute(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                 version    integer     PRIMARY KEY,
                 name       text        NOT NULL,
                 checksum   text        NOT NULL,
                 applied_at timestamptz NOT NULL DEFAULT now()
             )",
        )
        .await?;

    let applied: HashMap<i32, String> = client
        .query("SELECT version, checksum FROM schema_migrations", &[])
        .await?
        .iter()
        .map(|row| (row.get("version"), row.get("checksum")))
        .collect();

    for (&version, applied_checksum) in &applied {
        let Some(m) = migrations.iter().find(|m| m.version == version) else {
            return Err(MigrateError::Unknown { version });
        };
        let current = checksum(m.sql);
        if *applied_checksum != current {
            return Err(MigrateError::Edited {
                version,
                name: m.name.to_owned(),
                applied: applied_checksum.clone(),
                current,
            });
        }
    }

    let mut done = Vec::new();
    for m in migrations
        .iter()
        .filter(|m| !applied.contains_key(&m.version))
    {
        let tx = client.transaction().await?;
        tx.batch_execute(m.sql).await?;
        tx.execute(
            "INSERT INTO schema_migrations (version, name, checksum) VALUES ($1, $2, $3)",
            &[&m.version, &m.name, &checksum(m.sql)],
        )
        .await?;
        tx.commit().await?;
        tracing::info!(version = m.version, name = m.name, "applied migration");
        done.push(m.version);
    }
    if !done.is_empty() {
        // This connection may have looked up a type a migration has since changed.
        client.clear_type_cache();
    }
    Ok(done)
}

/// FNV-1a, 64 bits, as hex: enough to notice an edited file, and no dependency.
pub fn checksum(sql: &str) -> String {
    let hash = sql.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn checksum_is_stable_and_sensitive() {
        assert_eq!(super::checksum(""), "cbf29ce484222325");
        assert_ne!(super::checksum("SELECT 1"), super::checksum("SELECT 2"));
    }

    #[test]
    fn versions_are_ascending_and_unique() {
        let versions: Vec<i32> = super::MIGRATIONS.iter().map(|m| m.version).collect();
        assert!(versions.windows(2).all(|w| w[0] < w[1]), "{versions:?}");
    }
}

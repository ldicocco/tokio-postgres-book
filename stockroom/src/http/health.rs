//! Liveness, readiness and a look at the database's sessions.

use axum::Json;
use axum::extract::State;
use serde_json::{Value, json};

use super::{AppState, Db};
use crate::{AppError, repo};

/// The process is up. Deliberately does not touch the database: a database outage should
/// not make an orchestrator restart every instance of the service.
pub async fn live() -> &'static str {
    "ok"
}

/// The service can do its job: a connection can be had, and it answers.
pub async fn ready(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let db = state.pool.get().await?;
    db.check_connection().await?; // one round trip, no statement
    let pool = state.pool.status();
    Ok(Json(json!({
        "status": "ready",
        "pool": { "maxSize": pool.max_size, "size": pool.size, "available": pool.available, "waiting": pool.waiting },
        "listening": state.hub.server_info().is_some(),
    })))
}

pub async fn sessions(State(state): State<AppState>, Db(db): Db) -> Result<Json<Value>, AppError> {
    Ok(Json(json!({
        "server": state.hub.server_info(),
        "sessions": repo::reports::sessions(&db).await?,
    })))
}

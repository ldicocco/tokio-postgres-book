//! `/reports`

use axum::Json;
use axum::extract::State;
use jiff::civil::Date;
use serde::Deserialize;

use super::{AppQuery, AppState, Db};
use crate::AppError;
use crate::db::CancelOnDrop;
use crate::repo::reports::{self, Dashboard, SalesDay};

pub async fn dashboard(Db(db): Db) -> Result<Json<Dashboard>, AppError> {
    // `&db` is a `&Object`; deref coercion turns it into the `&tokio_postgres::Client` the
    // report needs for `query_one_scalar`.
    Ok(Json(reports::dashboard(&db).await?))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SalesQuery {
    from: Date,
    to: Date,
    /// Make the report artificially slow (milliseconds of `pg_sleep`), to watch the request
    /// timeout and the cancellation at work.
    #[serde(default)]
    delay_ms: u32,
}

/// A potentially slow report. If the request is abandoned (the router's timeout fires, or
/// the client disconnects) the handler's future is dropped; `CancelOnDrop` then cancels the
/// query on the server instead of letting it run on for nobody.
pub async fn sales(
    State(state): State<AppState>,
    Db(db): Db,
    AppQuery(query): AppQuery<SalesQuery>,
) -> Result<Json<Vec<SalesDay>>, AppError> {
    if query.to < query.from {
        return Err(AppError::invalid("to", "the period ends before it starts"));
    }
    if query.delay_ms > 60_000 {
        return Err(AppError::invalid("delayMs", "at most 60000"));
    }
    let guarded = CancelOnDrop::new(db, state.tls.clone());
    let result = reports::sales(&guarded, query.from, query.to, query.delay_ms).await;
    // Finished, successfully or not: nothing to cancel, and the connection goes back to the pool.
    drop(guarded.finish());
    Ok(Json(result?))
}

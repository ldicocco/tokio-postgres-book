//! The HTTP API (Chapters 15 and 16).

mod events;
mod health;
mod orders;
mod products;
mod reports;
mod stock;
mod stores;

use std::time::Duration;

use axum::Router;
use axum::extract::{FromRequest, FromRequestParts};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::routing::{get, post};
use deadpool_postgres::{Object, Pool};
use tokio_util::sync::CancellationToken;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use crate::AppError;
use crate::db::MakeTls;
use crate::notify::Hub;

/// What every handler can reach. Cheap to clone: each field is a handle.
#[derive(Clone)]
pub struct AppState {
    pub pool: Pool,
    pub hub: Hub,
    /// For cancel requests, which open a connection of their own.
    pub tls: MakeTls,
    /// Cancelled at shutdown; long-lived responses (SSE) end with it.
    pub shutdown: CancellationToken,
}

pub struct RouterOptions {
    /// How long a handler may take to produce its response (not to stream its body).
    pub request_timeout: Duration,
    /// The largest CSV `POST /products/import` accepts.
    pub import_limit: usize,
}

impl Default for RouterOptions {
    fn default() -> Self {
        RouterOptions {
            request_timeout: Duration::from_secs(10),
            import_limit: 64 * 1024 * 1024,
        }
    }
}

pub fn router(state: AppState, options: RouterOptions) -> Router {
    Router::new()
        .route("/healthz", get(health::live))
        .route("/readyz", get(health::ready))
        .route("/admin/sessions", get(health::sessions))
        .route("/stores", get(stores::list).post(stores::create))
        .route("/stores/{code}/stock", get(stock::levels))
        .route("/stores/{code}/stock/adjust", post(stock::adjust))
        .route("/stock/transfers", post(stock::transfer))
        .route("/products", get(products::list).post(products::create))
        .route("/products/batch", post(products::create_batch))
        .route("/products/stream", get(products::stream))
        .route("/products/export.csv", get(products::export_csv))
        .route(
            "/products/import",
            post(products::import_csv).layer(RequestBodyLimitLayer::new(options.import_limit)),
        )
        .route(
            "/products/{sku}",
            get(products::get).patch(products::update),
        )
        .route("/orders", post(orders::place))
        .route("/orders/events", get(events::orders))
        .route("/orders/{id}", get(orders::get))
        .route("/orders/{id}/transition", post(orders::transition))
        .route("/reports/dashboard", get(reports::dashboard))
        .route("/reports/sales", get(reports::sales))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::SERVICE_UNAVAILABLE,
            options.request_timeout,
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// A pooled connection, checked out for the duration of the request.
///
/// Extractors that read only the request head run before the body is read, so a handler
/// taking `Db` holds a connection while its body arrives: fine for small JSON bodies.
pub struct Db(pub Object);

impl FromRequestParts<AppState> for Db {
    type Rejection = AppError;

    async fn from_request_parts(
        _parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(Db(state.pool.get().await?))
    }
}

/// `axum::Json`, with its rejections in the API's error format.
#[derive(FromRequest)]
#[from_request(via(axum::Json), rejection(AppError))]
pub struct AppJson<T>(pub T);

/// `axum::extract::Query`, with its rejections in the API's error format.
#[derive(FromRequestParts)]
#[from_request(via(axum::extract::Query), rejection(AppError))]
pub struct AppQuery<T>(pub T);

/// A resource's version as an entity tag: `"3"`.
pub fn etag(version: i32) -> [(header::HeaderName, HeaderValue); 1] {
    let value =
        HeaderValue::from_str(&format!("\"{version}\"")).expect("digits and quotes are valid");
    [(header::ETAG, value)]
}

/// The version an `If-Match` header asks for: `None` without one (or with `*`).
pub fn if_match(headers: &HeaderMap) -> Result<Option<i32>, AppError> {
    let Some(value) = headers.get(header::IF_MATCH) else {
        return Ok(None);
    };
    let text = value.to_str().unwrap_or_default().trim();
    if text == "*" {
        return Ok(None);
    }
    text.trim_start_matches("W/")
        .trim_matches('"')
        .parse()
        .map(Some)
        .map_err(|_| AppError::invalid("If-Match", "expected an entity tag such as \"3\""))
}

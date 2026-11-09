//! The HTTP API: stockroom's, with the routes for stores, products and orders served by
//! handlers on shelf, and every other route by stockroom's own router, unchanged.
//!
//! The handlers are stockroom's (`stockroom/src/http/`), calling this crate's repository; the
//! extractors, the state and the error type are stockroom's too.

mod orders;
mod products;
mod stores;

use axum::Router;
use axum::http::StatusCode;
use axum::routing::{get, post};
use stockroom::http::{AppState, RouterOptions};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

/// Static paths of stockroom's that a ported route with a parameter would capture
/// (`/products/{sku}` matches `/products/import`), so they are routed to stockroom by name.
const STOCKROOM_PATHS: [&str; 3] = ["/products/export.csv", "/products/import", "/orders/events"];

pub fn router(state: AppState, options: RouterOptions) -> Router {
    let request_timeout = options.request_timeout;
    let stockroom = stockroom::http::router(state.clone(), options);
    let mut router = Router::new()
        .route("/stores", get(stores::list).post(stores::create))
        .route("/products", get(products::list).post(products::create))
        .route("/products/batch", post(products::create_batch))
        .route("/products/stream", get(products::stream))
        .route(
            "/products/{sku}",
            get(products::get).patch(products::update),
        )
        .route("/orders", post(orders::place))
        .route("/orders/{id}", get(orders::get))
        // Only on the routes above: stockroom's router has these layers of its own.
        .route_layer(TimeoutLayer::with_status_code(
            StatusCode::SERVICE_UNAVAILABLE,
            request_timeout,
        ))
        .route_layer(TraceLayer::new_for_http());
    // `into_service`: axum refuses a `Router` as a route's service, and asks for `nest`, which
    // would strip the path that stockroom's router routes by.
    for path in STOCKROOM_PATHS {
        router = router.route_service(path, stockroom.clone().into_service());
    }
    router.fallback_service(stockroom).with_state(state)
}

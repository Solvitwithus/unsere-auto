pub mod masters;
pub mod sales;
pub mod sync;
pub mod tickets;

use crate::state::AppState;
use axum::routing::get;
use axum::Router;

pub fn api() -> Router<AppState> {
    Router::new()
        .route("/api/health", get(|| async { axum::Json(serde_json::json!({ "ok": true, "service": "wb-server", "version": env!("CARGO_PKG_VERSION") })) }))
        .merge(masters::routes())
        .merge(tickets::routes())
        .merge(sales::routes())
        .merge(sync::routes())
}

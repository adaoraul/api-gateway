pub mod auth;
pub mod config;
pub mod error;
pub mod proxy;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::routing::any;
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use tower_http::trace::TraceLayer;

use crate::config::RuntimeConfig;

/// Shared, pooled HTTP(S) client used for upstream and authorization requests.
pub type HttpClient = Client<HttpsConnector<HttpConnector>, Body>;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<RuntimeConfig>,
    pub client: HttpClient,
}

pub fn build_client() -> HttpClient {
    // Idempotent; a second call (e.g. from tests) returns Err, which is fine.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let mut http = HttpConnector::new();
    http.enforce_http(false);
    http.set_connect_timeout(Some(CONNECT_TIMEOUT));

    let https = HttpsConnectorBuilder::new()
        .with_webpki_roots()
        .https_or_http()
        .enable_http1()
        .wrap_connector(http);

    Client::builder(TokioExecutor::new()).build(https)
}

/// Build the gateway router for the given configuration.
pub fn app(config: RuntimeConfig) -> Router {
    let state = AppState {
        config: Arc::new(config),
        client: build_client(),
    };
    Router::new()
        .route("/health-check", any(health_check))
        .fallback(proxy::proxy_handler)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn health_check() -> &'static str {
    "OK"
}

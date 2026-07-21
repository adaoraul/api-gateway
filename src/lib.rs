pub mod auth;
pub mod config;
pub mod error;
pub mod proxy;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderName, HeaderValue, Request};
use axum::routing::any;
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use tower_http::propagate_header::PropagateHeaderLayer;
use tower_http::request_id::{MakeRequestId, RequestId, SetRequestIdLayer};
use tower_http::trace::{DefaultOnResponse, TraceLayer};
use tracing::Level;
use uuid::Uuid;

use crate::config::RuntimeConfig;

const REQUEST_ID_HEADER: &str = "x-request-id";

/// Generates a UUIDv4 for each request that doesn't already carry an id
/// (e.g. from an upstream proxy like Caddy).
#[derive(Clone, Default)]
struct MakeUuidRequestId;

impl MakeRequestId for MakeUuidRequestId {
    fn make_request_id<B>(&mut self, _request: &Request<B>) -> Option<RequestId> {
        let header_value = HeaderValue::from_str(&Uuid::new_v4().to_string()).ok()?;
        Some(RequestId::new(header_value))
    }
}

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
    let request_id_header = HeaderName::from_static(REQUEST_ID_HEADER);

    Router::new()
        .route("/health-check", any(health_check))
        .fallback(proxy::proxy_handler)
        // Layers added later wrap those added earlier, so this stack runs,
        // per request, outer to inner: assign a request id (or keep one
        // supplied by an upstream proxy) -> open a span carrying it, logged
        // at INFO with status and latency -> copy the id back onto the
        // response, in that order.
        .layer(PropagateHeaderLayer::new(request_id_header.clone()))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &Request<Body>| {
                    let request_id = request
                        .extensions()
                        .get::<RequestId>()
                        .and_then(|id| id.header_value().to_str().ok())
                        .unwrap_or_default();
                    tracing::info_span!(
                        "request",
                        method = %request.method(),
                        path = %request.uri().path(),
                        request_id,
                    )
                })
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .layer(SetRequestIdLayer::new(request_id_header, MakeUuidRequestId))
        .with_state(state)
}

async fn health_check() -> &'static str {
    "OK"
}

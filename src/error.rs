use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// Errors surfaced to clients, each mapping to a specific HTTP status.
///
/// The distinction between `AuthorizationRejected` (the auth API said no)
/// and `AuthServiceUnavailable` (the auth API could not be reached) matters:
/// the former is the caller's problem (401), the latter is an infrastructure
/// problem (502).
#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error("no configured service matches the request path")]
    RouteNotFound,
    #[error("missing Authorization header")]
    MissingAuthorization,
    #[error("authorization rejected")]
    AuthorizationRejected,
    #[error("authorization service unavailable")]
    AuthServiceUnavailable,
    #[error("upstream service unavailable")]
    UpstreamUnavailable,
    #[error("upstream service timed out")]
    UpstreamTimeout,
    #[error("failed to build upstream request")]
    InvalidUpstreamRequest,
}

impl GatewayError {
    pub fn status(&self) -> StatusCode {
        match self {
            GatewayError::RouteNotFound => StatusCode::NOT_FOUND,
            GatewayError::MissingAuthorization | GatewayError::AuthorizationRejected => {
                StatusCode::UNAUTHORIZED
            }
            GatewayError::AuthServiceUnavailable | GatewayError::UpstreamUnavailable => {
                StatusCode::BAD_GATEWAY
            }
            GatewayError::UpstreamTimeout => StatusCode::GATEWAY_TIMEOUT,
            GatewayError::InvalidUpstreamRequest => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        (self.status(), self.to_string()).into_response()
    }
}

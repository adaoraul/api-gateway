use std::time::Duration;

use axum::body::Body;
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderValue, Request, Uri};
use tracing::{info, warn};

use crate::HttpClient;
use crate::error::GatewayError;

/// Ask the external authorization API whether this request may proceed.
///
/// The caller's Authorization header is forwarded as-is. A 2xx response
/// authorizes the request; a 4xx response rejects it (401); reachability
/// problems, timeouts, and 5xx responses map to 502 because they say nothing
/// about the caller's credentials.
pub async fn authorize(
    client: &HttpClient,
    auth_api_url: &Uri,
    auth_header: &HeaderValue,
    timeout: Duration,
) -> Result<(), GatewayError> {
    let request = Request::builder()
        .uri(auth_api_url.clone())
        .header(AUTHORIZATION, auth_header.clone())
        .body(Body::empty())
        .map_err(|_| GatewayError::AuthServiceUnavailable)?;

    let response = tokio::time::timeout(timeout, client.request(request))
        .await
        .map_err(|_| {
            warn!(url = %auth_api_url, "authorization API timed out");
            GatewayError::AuthServiceUnavailable
        })?
        .map_err(|error| {
            warn!(url = %auth_api_url, %error, "could not reach authorization API");
            GatewayError::AuthServiceUnavailable
        })?;

    let status = response.status();
    if status.is_success() {
        info!("authorization successful");
        Ok(())
    } else if status.is_server_error() {
        warn!(%status, "authorization API returned a server error");
        Err(GatewayError::AuthServiceUnavailable)
    } else {
        info!(%status, "authorization rejected");
        Err(GatewayError::AuthorizationRejected)
    }
}

use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::header::{AUTHORIZATION, CONNECTION, HOST};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Uri};
use axum::response::{IntoResponse, Response};
use tracing::{info, warn};

use crate::AppState;
use crate::auth;
use crate::config::Route;
use crate::error::GatewayError;

/// Headers that only apply to a single connection and must not be forwarded.
const HOP_BY_HOP_HEADERS: [&str; 8] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

pub async fn proxy_handler(
    State(state): State<AppState>,
    ConnectInfo(client_addr): ConnectInfo<SocketAddr>,
    request: Request,
) -> Response {
    match handle(state, client_addr, request).await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}

async fn handle(
    state: AppState,
    client_addr: SocketAddr,
    request: Request,
) -> Result<Response, GatewayError> {
    let path = request.uri().path().to_owned();
    let route = state.config.find_route(&path).ok_or_else(|| {
        warn!(%path, "no route matches path");
        GatewayError::RouteNotFound
    })?;

    if route.requires_auth {
        // Presence of the URL is enforced during config validation.
        let auth_api_url = state
            .config
            .authorization_api_url
            .as_ref()
            .ok_or(GatewayError::AuthServiceUnavailable)?;
        let auth_header = request
            .headers()
            .get(AUTHORIZATION)
            .ok_or(GatewayError::MissingAuthorization)?;
        auth::authorize(
            &state.client,
            auth_api_url,
            auth_header,
            state.config.timeout,
        )
        .await?;
    }

    forward(&state, client_addr, route, request).await
}

/// Stream the request to the routed upstream and stream its response back.
async fn forward(
    state: &AppState,
    client_addr: SocketAddr,
    route: &Route,
    request: Request,
) -> Result<Response, GatewayError> {
    let (parts, body) = request.into_parts();
    let method = parts.method;
    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");
    let uri: Uri = format!("{}{}", route.upstream_base, path_and_query)
        .parse()
        .map_err(|_| GatewayError::InvalidUpstreamRequest)?;

    let mut headers = parts.headers;
    strip_hop_by_hop_headers(&mut headers);
    // The client sets `Host` from the upstream URI; keep the original value
    // available to the backend the standard way.
    if let Some(host) = headers.remove(HOST) {
        headers.insert(HeaderName::from_static("x-forwarded-host"), host);
    }
    append_forwarded_for(&mut headers, client_addr);
    headers.insert(
        HeaderName::from_static("x-forwarded-proto"),
        HeaderValue::from_static("http"),
    );

    let mut upstream_request = Request::builder()
        .method(method.clone())
        .uri(uri)
        .body(body)
        .map_err(|_| GatewayError::InvalidUpstreamRequest)?;
    *upstream_request.headers_mut() = headers;

    info!(%method, path = %path_and_query, upstream = %route.upstream_base, "forwarding request");

    let response =
        tokio::time::timeout(state.config.timeout, state.client.request(upstream_request))
            .await
            .map_err(|_| {
                warn!(upstream = %route.upstream_base, "upstream timed out");
                GatewayError::UpstreamTimeout
            })?
            .map_err(|error| {
                warn!(upstream = %route.upstream_base, %error, "could not reach upstream");
                GatewayError::UpstreamUnavailable
            })?;

    let (mut parts, body) = response.into_parts();
    strip_hop_by_hop_headers(&mut parts.headers);
    Ok(Response::from_parts(parts, Body::new(body)))
}

/// Remove hop-by-hop headers, including any named by the `Connection` header.
fn strip_hop_by_hop_headers(headers: &mut HeaderMap) {
    let connection_listed: Vec<HeaderName> = headers
        .get_all(CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|name| HeaderName::try_from(name.trim()).ok())
        .collect();
    for name in connection_listed {
        headers.remove(&name);
    }
    for name in HOP_BY_HOP_HEADERS {
        headers.remove(name);
    }
}

fn append_forwarded_for(headers: &mut HeaderMap, client_addr: SocketAddr) {
    let client_ip = client_addr.ip().to_string();
    let name = HeaderName::from_static("x-forwarded-for");
    let value = match headers.get(&name).and_then(|value| value.to_str().ok()) {
        Some(existing) => format!("{existing}, {client_ip}"),
        None => client_ip,
    };
    if let Ok(value) = HeaderValue::from_str(&value) {
        headers.insert(name, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header_map(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.append(
                HeaderName::try_from(*name).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        headers
    }

    #[test]
    fn strips_standard_hop_by_hop_headers() {
        let mut headers = header_map(&[
            ("connection", "keep-alive"),
            ("keep-alive", "timeout=5"),
            ("transfer-encoding", "chunked"),
            ("upgrade", "websocket"),
            ("content-type", "application/json"),
        ]);
        strip_hop_by_hop_headers(&mut headers);
        assert_eq!(headers.len(), 1);
        assert!(headers.contains_key("content-type"));
    }

    #[test]
    fn strips_headers_named_by_the_connection_header() {
        let mut headers = header_map(&[
            ("connection", "x-custom-a, x-custom-b"),
            ("x-custom-a", "1"),
            ("x-custom-b", "2"),
            ("x-kept", "3"),
        ]);
        strip_hop_by_hop_headers(&mut headers);
        assert!(!headers.contains_key("x-custom-a"));
        assert!(!headers.contains_key("x-custom-b"));
        assert!(headers.contains_key("x-kept"));
    }

    #[test]
    fn sets_forwarded_for_when_absent() {
        let mut headers = HeaderMap::new();
        append_forwarded_for(&mut headers, "10.0.0.9:1234".parse().unwrap());
        assert_eq!(headers.get("x-forwarded-for").unwrap(), "10.0.0.9");
    }

    #[test]
    fn appends_to_an_existing_forwarded_for_chain() {
        let mut headers = header_map(&[("x-forwarded-for", "203.0.113.7")]);
        append_forwarded_for(&mut headers, "10.0.0.9:1234".parse().unwrap());
        assert_eq!(
            headers.get("x-forwarded-for").unwrap(),
            "203.0.113.7, 10.0.0.9"
        );
    }
}

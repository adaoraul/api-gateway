use std::net::SocketAddr;
use std::time::Duration;

use axum::Router;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::routing::any;
use serde_json::{Map, Value, json};
use tokio::net::TcpListener;

/// Serve a router on an ephemeral port and return its address.
async fn serve(router: Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    addr
}

async fn spawn_gateway(config_toml: &str) -> SocketAddr {
    let config = vordr::config::parse(config_toml).expect("invalid test config");
    serve(vordr::app(config)).await
}

/// Upstream that echoes the request back as JSON.
async fn echo(request: Request) -> axum::Json<Value> {
    let (parts, body) = request.into_parts();
    let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    let headers: Map<String, Value> = parts
        .headers
        .iter()
        .map(|(name, value)| {
            (
                name.to_string(),
                Value::from(String::from_utf8_lossy(value.as_bytes()).to_string()),
            )
        })
        .collect();
    axum::Json(json!({
        "method": parts.method.as_str(),
        "uri": parts.uri.to_string(),
        "headers": headers,
        "body": String::from_utf8_lossy(&body),
    }))
}

async fn spawn_echo_upstream() -> SocketAddr {
    serve(Router::new().fallback(echo)).await
}

/// Auth API stub that answers every request with the given status.
async fn spawn_auth_api(status: StatusCode) -> SocketAddr {
    serve(Router::new().fallback(any(move || async move { status }))).await
}

/// An address nothing is listening on.
fn unreachable_addr() -> SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    addr
}

fn public_route_config(upstream: SocketAddr) -> String {
    format!(
        r#"
        [[services]]
        path = "^/api"
        target_service = "http://{upstream}"
        authentication_required = false
        "#
    )
}

fn auth_route_config(auth_api: SocketAddr, upstream: SocketAddr) -> String {
    format!(
        r#"
        authorization_api_url = "http://{auth_api}/authorize"

        [[services]]
        path = "^/api"
        target_service = "http://{upstream}"
        "#
    )
}

#[tokio::test]
async fn health_check_returns_ok() {
    let gateway = spawn_gateway("services = []").await;

    let response = reqwest::get(format!("http://{gateway}/health-check"))
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    assert_eq!(response.text().await.unwrap(), "OK");
}

#[tokio::test]
async fn forwards_method_path_query_and_body() {
    let upstream = spawn_echo_upstream().await;
    let gateway = spawn_gateway(&public_route_config(upstream)).await;

    let response = reqwest::Client::new()
        .post(format!("http://{gateway}/api/users/42?page=2&sort=name"))
        .body("hello upstream")
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let echoed: Value = response.json().await.unwrap();
    assert_eq!(echoed["method"], "POST");
    assert_eq!(echoed["uri"], "/api/users/42?page=2&sort=name");
    assert_eq!(echoed["body"], "hello upstream");
}

#[tokio::test]
async fn unmatched_path_returns_404() {
    let upstream = spawn_echo_upstream().await;
    let gateway = spawn_gateway(&public_route_config(upstream)).await;

    let response = reqwest::get(format!("http://{gateway}/nope"))
        .await
        .unwrap();

    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn missing_authorization_header_returns_401_without_calling_upstream() {
    let auth_api = spawn_auth_api(StatusCode::OK).await;
    // If the gateway (incorrectly) forwarded the request anyway, the
    // unreachable upstream would turn this into a 502 instead of a 401.
    let gateway = spawn_gateway(&auth_route_config(auth_api, unreachable_addr())).await;

    let response = reqwest::get(format!("http://{gateway}/api/users"))
        .await
        .unwrap();

    assert_eq!(response.status(), 401);
}

#[tokio::test]
async fn rejected_token_returns_401() {
    let auth_api = spawn_auth_api(StatusCode::FORBIDDEN).await;
    let upstream = spawn_echo_upstream().await;
    let gateway = spawn_gateway(&auth_route_config(auth_api, upstream)).await;

    let response = reqwest::Client::new()
        .get(format!("http://{gateway}/api/users"))
        .header("authorization", "Bearer expired")
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 401);
}

#[tokio::test]
async fn valid_token_is_forwarded_to_upstream() {
    let auth_api = spawn_auth_api(StatusCode::OK).await;
    let upstream = spawn_echo_upstream().await;
    let gateway = spawn_gateway(&auth_route_config(auth_api, upstream)).await;

    let response = reqwest::Client::new()
        .get(format!("http://{gateway}/api/users"))
        .header("authorization", "Bearer test-token")
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let echoed: Value = response.json().await.unwrap();
    assert_eq!(echoed["headers"]["authorization"], "Bearer test-token");
}

#[tokio::test]
async fn public_route_preserves_client_authorization_header() {
    let upstream = spawn_echo_upstream().await;
    let gateway = spawn_gateway(&public_route_config(upstream)).await;

    let response = reqwest::Client::new()
        .get(format!("http://{gateway}/api/users"))
        .header("authorization", "Bearer pass-through")
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let echoed: Value = response.json().await.unwrap();
    assert_eq!(echoed["headers"]["authorization"], "Bearer pass-through");
}

#[tokio::test]
async fn unreachable_auth_api_returns_502() {
    let upstream = spawn_echo_upstream().await;
    let gateway = spawn_gateway(&auth_route_config(unreachable_addr(), upstream)).await;

    let response = reqwest::Client::new()
        .get(format!("http://{gateway}/api/users"))
        .header("authorization", "Bearer token")
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 502);
}

#[tokio::test]
async fn unreachable_upstream_returns_502() {
    let gateway = spawn_gateway(&public_route_config(unreachable_addr())).await;

    let response = reqwest::get(format!("http://{gateway}/api/users"))
        .await
        .unwrap();

    assert_eq!(response.status(), 502);
}

#[tokio::test]
async fn slow_upstream_returns_504() {
    let upstream = serve(Router::new().fallback(any(|| async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        "too late"
    })))
    .await;
    let config = format!("timeout_seconds = 1\n{}", public_route_config(upstream));
    let gateway = spawn_gateway(&config).await;

    let response = reqwest::get(format!("http://{gateway}/api/users"))
        .await
        .unwrap();

    assert_eq!(response.status(), 504);
}

#[tokio::test]
async fn forwarded_headers_are_set_and_hop_by_hop_headers_are_stripped() {
    let upstream = spawn_echo_upstream().await;
    let gateway = spawn_gateway(&public_route_config(upstream)).await;

    let response = reqwest::get(format!("http://{gateway}/api/users"))
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let echoed: Value = response.json().await.unwrap();
    let headers = echoed["headers"].as_object().unwrap();
    assert_eq!(headers["x-forwarded-for"], "127.0.0.1");
    assert_eq!(headers["x-forwarded-proto"], "http");
    assert_eq!(headers["x-forwarded-host"], gateway.to_string());
    // The upstream sees the Host of its own URI, not the gateway's.
    assert_eq!(headers["host"], upstream.to_string());
    assert!(!headers.contains_key("connection"));
    assert!(!headers.contains_key("transfer-encoding"));
}

#[tokio::test]
async fn preserves_x_forwarded_proto_set_by_a_fronting_proxy() {
    let upstream = spawn_echo_upstream().await;
    let gateway = spawn_gateway(&public_route_config(upstream)).await;

    // A TLS-terminating proxy in front (e.g. Caddy) would set this to
    // "https" for the original client connection; vordr's own listener only
    // ever speaks plain HTTP, so it must not overwrite an existing value.
    let response = reqwest::Client::new()
        .get(format!("http://{gateway}/api/users"))
        .header("x-forwarded-proto", "https")
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let echoed: Value = response.json().await.unwrap();
    assert_eq!(echoed["headers"]["x-forwarded-proto"], "https");
}

#[tokio::test]
async fn response_carries_a_request_id_header() {
    let upstream = spawn_echo_upstream().await;
    let gateway = spawn_gateway(&public_route_config(upstream)).await;

    let response = reqwest::get(format!("http://{gateway}/api/users"))
        .await
        .unwrap();

    let request_id = response
        .headers()
        .get("x-request-id")
        .expect("response should carry a request id")
        .to_str()
        .unwrap();
    assert!(!request_id.is_empty());
}

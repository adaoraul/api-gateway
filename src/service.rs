use hyper::header::HeaderValue;
use hyper::http::request::Parts;
use hyper::{Body, Client, Request, Response};
use reqwest::header::{HeaderMap, AUTHORIZATION};
use tracing::{info, warn};

use crate::config::{GatewayConfig, ServiceConfig};

// Handle incoming requests
pub async fn handle_request(
    req: Request<Body>,
    config: GatewayConfig,
) -> Result<Response<Body>, hyper::Error> {
    // Get the requested path
    let path = req.uri().path();

    // Log that an incoming request was received
    info!("Incoming request for path: {}", path);

    // Check if the requested path is the health-check endpoint
    if path == "/health-check" {
        return health_check();
    }

    // Get the service configuration for the requested path
    let service_config = match get_service_config(path.clone(), &config.services) {
        Some(service_config) => service_config,
        None => {
            // If no service configuration exists for the requested path, return a 404 response
            warn!("Path not found: {}", path);
            return not_found();
        }
    };

    // Check if the requested service requires authentication
    let auth_token = if service_config.authentication_required.unwrap_or(true) {
        // If so, authorize the user by sending a request to the authorization API
        match authorize_user(&req.headers(), &config.authorization_api_url).await {
            Ok(header) => header,
            Err(_) => {
                // If there is an error connecting to the authorization API, return a 503 response
                warn!("Failed to connect to Authorization API");
                return service_unavailable("Failed to connect to Authorization API");
            }
        }
    } else {
        String::new()
    };

    // Build the downstream request
    let (parts, body) = req.into_parts();
    let downstream_req = build_downstream_request(parts, body, service_config, auth_token).await?;

    // Forward the request to the requested service
    match forward_request(downstream_req).await {
        Ok(res) => {
            // If the request is successful, log that it was forwarded and return the response
            info!("Forwarded request successfully");
            Ok(res)
        }
        Err(_) => {
            // If there is an error connecting to the requested service, return a 503 response
            warn!("Failed to connect to downstream service");
            service_unavailable("Failed to connect to downstream service")
        }
    }
}

// Get the service configuration for the requested path
fn get_service_config<'a>(path: &str, services: &'a [ServiceConfig]) -> Option<&'a ServiceConfig> {
    services.iter().find(|c| c.path.is_match(path))
}

// Authorize the user by sending a request to the authorization API
async fn authorize_user(headers: &HeaderMap, auth_api_url: &str) -> Result<String, ()> {
    let auth_header_value = match headers.get(AUTHORIZATION) {
        Some(value) => value.to_str().unwrap_or_default(),
        None => {
            // If the authorization header is missing, log a warning and return an empty string
            warn!("Authorization header not found");
            ""
        }
    };

    let auth_request = reqwest::Client::new()
        .get(auth_api_url)
        .header(AUTHORIZATION, auth_header_value);

    match auth_request.send().await {
        Ok(res) if res.status().is_success() => {
            // If the authorization request is successful, log that it was successful and return the authorization header
            info!("Authorization successful");
            Ok(auth_header_value.to_string())
        }
        _ => {
            // If the authorization request is unsuccessful, log a warning and return an error
            warn!("Authorization failed");
            Err(())
        }
    }
}

// Build the downstream request
async fn build_downstream_request(
    parts: Parts,
    body: Body,
    service_config: &ServiceConfig,
    auth_token: String,
) -> Result<Request<Body>, hyper::Error> {
    let req = Request::from_parts(parts, body);
    let uri = format!(
        "{}:{}{}",
        service_config.target_service,
        service_config.target_port,
        req.uri().path()
    );

    let mut downstream_req_builder = Request::builder()
        .uri(uri)
        .method(req.method())
        .version(req.version());

    *downstream_req_builder.headers_mut().unwrap() = req.headers().clone();

    downstream_req_builder
        .headers_mut()
        .unwrap()
        .insert("Authorization", HeaderValue::from_str(&auth_token).unwrap());

    let body_bytes = hyper::body::to_bytes(req.into_body()).await?;

    // Log that the downstream request is being built and return the completed request
    info!("Building downstream request");
    let downstream_req = downstream_req_builder.body(Body::from(body_bytes));
    Ok(downstream_req.unwrap())
}

// Forward the request to the requested service
async fn forward_request(req: Request<Body>) -> Result<Response<Body>, ()> {
    match Client::new().request(req).await {
        Ok(res) => {
            // If the request is successful, log that it was successful and return the response
            info!("Request forwarded successfully");
            Ok(res)
        }
        Err(_) => {
            // If there is an error connecting to the requested service, return an error
            warn!("Failed to forward request");
            Err(())
        }
    }
}

// Return a 200 response for the health check
fn health_check() -> Result<Response<Body>, hyper::Error> {
    let response = Response::new(Body::from("OK"));
    info!("Responding with 200 OK for health check");
    Ok(response)
}

// Return a 404 response
fn not_found() -> Result<Response<Body>, hyper::Error> {
    let mut response = Response::new(Body::from("404 Not Found"));
    *response.status_mut() = hyper::StatusCode::NOT_FOUND;
    warn!("Responding with 404 Not Found");
    Ok(response)
}

// Return a 503 response with a reason
fn service_unavailable<T>(reason: T) -> Result<Response<Body>, hyper::Error>
where
    T: Into<Body>,
{
    let mut response = Response::new(reason.into());
    *response.status_mut() = hyper::StatusCode::SERVICE_UNAVAILABLE;
    warn!("Responding with 503 Service Unavailable");
    Ok(response)
}

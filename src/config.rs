use std::net::SocketAddr;
use std::time::Duration;

use axum::http::Uri;
use regex::Regex;
use serde::Deserialize;

pub const DEFAULT_LISTEN_ADDRESS: &str = "0.0.0.0:8080";
pub const DEFAULT_TIMEOUT_SECONDS: u64 = 30;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not read config file: {0}")]
    Read(#[from] std::io::Error),
    #[error("invalid TOML: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("{0}")]
    Invalid(String),
}

/// Configuration as written in the TOML file. Field names are backward
/// compatible with pre-0.2 config files.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    listen_address: Option<String>,
    authorization_api_url: Option<String>,
    timeout_seconds: Option<u64>,
    services: Vec<RawService>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawService {
    path: String,
    target_service: String,
    target_port: Option<Port>,
    authentication_required: Option<bool>,
}

/// Accepts both `target_port = 8080` and the legacy `target_port = "8080"`.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Port {
    Number(u16),
    Text(String),
}

/// Validated configuration, ready to serve requests.
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub listen_address: SocketAddr,
    pub authorization_api_url: Option<Uri>,
    pub timeout: Duration,
    pub routes: Vec<Route>,
}

#[derive(Debug, Clone)]
pub struct Route {
    pub pattern: Regex,
    /// `scheme://host[:port]`, without a trailing slash.
    pub upstream_base: String,
    pub requires_auth: bool,
}

impl RuntimeConfig {
    pub fn find_route(&self, path: &str) -> Option<&Route> {
        self.routes
            .iter()
            .find(|route| route.pattern.is_match(path))
    }
}

pub fn load(path: &str) -> Result<RuntimeConfig, ConfigError> {
    let contents = std::fs::read_to_string(path)?;
    parse(&contents)
}

pub fn parse(contents: &str) -> Result<RuntimeConfig, ConfigError> {
    let raw: RawConfig = toml::from_str(contents)?;
    validate(raw)
}

fn invalid(message: impl Into<String>) -> ConfigError {
    ConfigError::Invalid(message.into())
}

fn validate(raw: RawConfig) -> Result<RuntimeConfig, ConfigError> {
    let listen_address = raw
        .listen_address
        .as_deref()
        .unwrap_or(DEFAULT_LISTEN_ADDRESS);
    let listen_address: SocketAddr = listen_address.parse().map_err(|_| {
        invalid(format!(
            "`listen_address` is not a valid socket address: `{listen_address}` \
             (expected something like \"0.0.0.0:8080\")"
        ))
    })?;

    let authorization_api_url = raw
        .authorization_api_url
        .map(|url| parse_http_url(&url, "authorization_api_url"))
        .transpose()?;

    let timeout_seconds = raw.timeout_seconds.unwrap_or(DEFAULT_TIMEOUT_SECONDS);
    if timeout_seconds == 0 {
        return Err(invalid("`timeout_seconds` must be greater than zero"));
    }

    let routes = raw
        .services
        .into_iter()
        .map(validate_service)
        .collect::<Result<Vec<_>, _>>()?;

    if authorization_api_url.is_none()
        && let Some(route) = routes.iter().find(|route| route.requires_auth)
    {
        return Err(invalid(format!(
            "`authorization_api_url` is required because the service matching `{}` \
             requires authentication (services require authentication unless \
             `authentication_required = false` is set)",
            route.pattern.as_str()
        )));
    }

    Ok(RuntimeConfig {
        listen_address,
        authorization_api_url,
        timeout: Duration::from_secs(timeout_seconds),
        routes,
    })
}

fn validate_service(raw: RawService) -> Result<Route, ConfigError> {
    // Anchor patterns at the start of the path so `path = "/users"` cannot
    // accidentally match `/api/users`.
    let pattern_source = if raw.path.starts_with('^') {
        raw.path.clone()
    } else {
        format!("^{}", raw.path)
    };
    let pattern = Regex::new(&pattern_source)
        .map_err(|error| invalid(format!("invalid `path` regex `{}`: {error}", raw.path)))?;

    let uri = parse_http_url(&raw.target_service, "target_service")?;
    if uri.path() != "/" && !uri.path().is_empty() {
        return Err(invalid(format!(
            "`target_service` must not include a path, got `{}`",
            raw.target_service
        )));
    }

    let port = match raw.target_port {
        Some(port) => {
            if uri.port_u16().is_some() {
                return Err(invalid(format!(
                    "service `{}` sets a port in both `target_service` and `target_port`; \
                     use one or the other",
                    raw.target_service
                )));
            }
            let port = match port {
                Port::Number(number) => number,
                Port::Text(text) => text
                    .parse::<u16>()
                    .map_err(|_| invalid(format!("`target_port` is not a valid port: `{text}`")))?,
            };
            Some(port)
        }
        None => uri.port_u16(),
    };

    // `parse_http_url` guarantees scheme and host are present.
    let scheme = uri.scheme_str().unwrap_or("http");
    let host = uri.host().unwrap_or_default();
    let upstream_base = match port {
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    };

    Ok(Route {
        pattern,
        upstream_base,
        requires_auth: raw.authentication_required.unwrap_or(true),
    })
}

fn parse_http_url(url: &str, field: &str) -> Result<Uri, ConfigError> {
    let uri: Uri = url
        .parse()
        .map_err(|_| invalid(format!("`{field}` is not a valid URL: `{url}`")))?;
    match uri.scheme_str() {
        Some("http") | Some("https") => {}
        _ => {
            return Err(invalid(format!(
                "`{field}` must start with http:// or https://, got `{url}`"
            )));
        }
    }
    if uri.host().is_none() {
        return Err(invalid(format!(
            "`{field}` must include a host, got `{url}`"
        )));
    }
    Ok(uri)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEGACY_CONFIG: &str = r#"
        authorization_api_url = "https://auth.example.com/api/v1/authorization"

        [[services]]
        path = "^/users"
        target_service = "http://user-service.default.svc.cluster.local"
        target_port = "8080"
        authentication_required = false

        [[services]]
        path = "^/orders"
        target_service = "http://order-service.default.svc.cluster.local"
        target_port = 8080
    "#;

    #[test]
    fn legacy_config_still_parses() {
        let config = parse(LEGACY_CONFIG).unwrap();
        assert_eq!(config.listen_address.to_string(), DEFAULT_LISTEN_ADDRESS);
        assert_eq!(config.timeout, Duration::from_secs(DEFAULT_TIMEOUT_SECONDS));
        assert_eq!(config.routes.len(), 2);
        assert_eq!(
            config.routes[0].upstream_base,
            "http://user-service.default.svc.cluster.local:8080"
        );
        assert!(!config.routes[0].requires_auth);
        // `authentication_required` defaults to true when omitted.
        assert!(config.routes[1].requires_auth);
    }

    #[test]
    fn port_can_come_from_target_service_url() {
        let config = parse(
            r#"
            [[services]]
            path = "^/api"
            target_service = "http://127.0.0.1:9000"
            authentication_required = false
            "#,
        )
        .unwrap();
        assert_eq!(config.routes[0].upstream_base, "http://127.0.0.1:9000");
    }

    #[test]
    fn unanchored_patterns_are_anchored_to_path_start() {
        let config = parse(
            r#"
            [[services]]
            path = "/users"
            target_service = "http://localhost:1"
            authentication_required = false
            "#,
        )
        .unwrap();
        assert!(config.find_route("/users/42").is_some());
        assert!(config.find_route("/api/users").is_none());
    }

    #[test]
    fn auth_url_required_when_any_service_requires_auth() {
        let error = parse(
            r#"
            [[services]]
            path = "^/orders"
            target_service = "http://localhost:1"
            "#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("authorization_api_url"));
    }

    #[test]
    fn conflicting_ports_are_rejected() {
        let error = parse(
            r#"
            [[services]]
            path = "^/api"
            target_service = "http://localhost:9000"
            target_port = 9001
            authentication_required = false
            "#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("target_port"));
    }

    #[test]
    fn invalid_regex_is_rejected() {
        let error = parse(
            r#"
            [[services]]
            path = "^/users["
            target_service = "http://localhost:1"
            authentication_required = false
            "#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("regex"));
    }

    #[test]
    fn zero_timeout_is_rejected() {
        let error = parse(
            r#"
            timeout_seconds = 0
            services = []
            "#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("timeout_seconds"));
    }
}

# Vordr

**Vordr** (Old Norse *vörðr*, "warden" — a guardian spirit that walks ahead of
a person) is a small, single-binary API gateway written in Rust (axum /
hyper 1.x). It routes requests to backend services by matching the request path
against regex patterns, and can delegate authentication to an external
authorization API.

It is designed for two audiences:

- **Local development** — run one tiny binary in front of your services so your
  apps can be tested against a real gateway instead of a mock.
- **Small-scale deployments** — a pragmatic gateway for projects that don't
  need (or can't afford) heavyweight infrastructure.

## Features

- Path-based routing with regex patterns (anchored to the start of the path)
- Streaming proxy — request and response bodies are never buffered in memory
- Optional per-service authentication delegated to an external authorization API
- HTTP and HTTPS backends, with a shared connection pool
- Correct proxy behavior: query strings preserved, hop-by-hop headers stripped,
  `X-Forwarded-For` / `X-Forwarded-Proto` / `X-Forwarded-Host` set
- Configurable timeouts (`504` on upstream timeout) and listen address
- Structured logging via `tracing`, with a request id correlating every log
  line for a request (`RUST_LOG` controls verbosity, `VORDR_LOG_FORMAT=json`
  for machine-readable output)
- Graceful shutdown on Ctrl+C / SIGTERM
- `/health-check` endpoint always answered by the gateway itself

## Getting started

Requires a recent stable Rust toolchain.

```bash
git clone https://github.com/adaoraul/vordr.git
cd vordr
cp config.example.toml config.toml   # then edit it
cargo build --release
./target/release/vordr               # or: ./target/release/vordr path/to/config.toml
```

The config file path can also be set with the `VORDR_CONFIG` environment
variable; the first CLI argument wins, and the default is `./config.toml`.

## Configuration

See [config.example.toml](config.example.toml) for a commented example.

| Key | Required | Default | Description |
| --- | --- | --- | --- |
| `listen_address` | no | `0.0.0.0:8080` | Address and port the gateway listens on. |
| `authorization_api_url` | only if a service requires auth | — | URL that authorization checks are sent to. |
| `timeout_seconds` | no | `30` | Per-request upstream timeout in seconds. |
| `services` | yes | — | Array of routed services (see below). |

Each `[[services]]` entry:

| Key | Required | Default | Description |
| --- | --- | --- | --- |
| `path` | yes | — | Regex matched against the request path. Anchored to the path start automatically. First matching service wins, in file order. |
| `target_service` | yes | — | Backend base URL: `http://host`, `https://host`, or `http://host:port`. |
| `target_port` | no | from `target_service`, else scheme default | Backend port; number or string. Cannot be combined with a port in `target_service`. |
| `authentication_required` | no | **`true`** | Whether the request must be authorized before forwarding. |

Configuration is validated at startup: bad regexes, malformed URLs, port
conflicts, and a missing `authorization_api_url` (when a service needs auth)
are reported with a clear error instead of a panic.

## How authentication works

For services with `authentication_required = true`, the gateway forwards the
caller's `Authorization` header to `authorization_api_url` via a `GET` request:

- **2xx** from the auth API → the request is forwarded to the backend, with the
  original `Authorization` header intact.
- **4xx** from the auth API → `401 Unauthorized`.
- Missing `Authorization` header → `401 Unauthorized` (the auth API is not called).
- Auth API unreachable, timed out, or **5xx** → `502 Bad Gateway`.

Services with `authentication_required = false` are forwarded untouched — the
caller's `Authorization` header is passed through to the backend.

## Response status codes

| Status | Meaning |
| --- | --- |
| `404` | No configured service matches the path. |
| `401` | Missing `Authorization` header, or the auth API rejected it. |
| `502` | Auth API or backend unreachable (or auth API 5xx). |
| `504` | Backend did not respond within `timeout_seconds`. |
| anything else | Returned by the backend, passed through unchanged. |

## Logging

Every request gets a `request_id` (a UUIDv4, or one already set by an
upstream proxy) that's attached to every log line for that request and
echoed back on the `X-Request-Id` response header, so you can grep one
request's full story out of concurrent traffic. `RUST_LOG` controls
verbosity (`info` by default; try `debug` for per-request detail beyond the
access-log line). Set `VORDR_LOG_FORMAT=json` for structured, one-line-per-
event JSON output suited to log aggregators.

## Development

```bash
cargo test                                 # unit + integration tests (all in-process)
cargo fmt --all --check                    # formatting
cargo clippy --all-targets -- -D warnings  # lints
```

CI runs the same three commands on every push and pull request.

## License

This project is licensed under the MIT License. See the [LICENSE](LICENSE) file
for details.

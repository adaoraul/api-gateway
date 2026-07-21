# syntax=docker/dockerfile:1

FROM rust:1-slim-bookworm AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

# rustls uses the webpki-roots crate (bundled Mozilla CA store), so no
# ca-certificates package is needed at runtime to validate https:// upstreams.
FROM debian:bookworm-slim AS runtime
RUN useradd --system --no-create-home --shell /usr/sbin/nologin vordr
COPY --from=builder /build/target/release/vordr /usr/local/bin/vordr
USER vordr
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/vordr"]
CMD ["/etc/vordr/config.toml"]

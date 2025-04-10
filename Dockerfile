FROM rust:1.86.0-slim-bookworm AS builder

WORKDIR /app

COPY .sqlx ./.sqlx/
COPY Cargo.toml Cargo.lock ./
COPY src ./src

# Installiere OpenSSL-Entwicklungspakete
ENV SQLX_OFFLINE=true

RUN apt-get update && apt-get install -y --no-install-recommends libssl-dev pkg-config

# Baue Abhängigkeiten
RUN cargo build --release --target x86_64-unknown-linux-gnu

# Final Stage
FROM debian:bookworm-slim AS runtime

WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends libssl-dev pkg-config ca-certificates

COPY default.config.toml ./

COPY --from=builder /app/target/x86_64-unknown-linux-gnu/release/ism ./

ENV RUST_LOG=info
ENV ISM_MODE=production

EXPOSE 5403

CMD ["./ism"]
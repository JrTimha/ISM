FROM rust:1.91.0-slim-bookworm AS builder

WORKDIR /app

COPY .sqlx ./.sqlx/
COPY Cargo.toml Cargo.lock ./
COPY src ./src

ENV SQLX_OFFLINE=true

# Installiere OpenSSL-Entwicklungspakete
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    libssl-dev \
    pkg-config \
    cmake


# Baue Abhängigkeiten
RUN cargo build --release --target x86_64-unknown-linux-gnu

# Final Stage
FROM debian:bookworm-slim AS runtime

RUN groupadd --system --gid 1001 ism && \
    useradd --system --uid 1001 --gid ism ism

RUN apt-get update && apt-get install -y --no-install-recommends \
    libssl3 \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*


WORKDIR /app

COPY default.config.toml ./
COPY --from=builder /app/target/x86_64-unknown-linux-gnu/release/ism ./

RUN chown -R ism:ism /app
USER ism

ENV RUST_LOG=info
ENV ISM_MODE=production

EXPOSE 5403

CMD ["./ism"]
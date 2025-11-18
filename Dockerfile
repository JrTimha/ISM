FROM rust:1.91.0-slim-bookworm AS builder

WORKDIR /app

COPY .sqlx ./.sqlx/
COPY Cargo.toml Cargo.lock ./
COPY src ./src

ENV SQLX_OFFLINE=true

# Install package requirements
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    libssl-dev \
    pkg-config \
    cmake

# compile ism
RUN cargo build --release

# Final Stage
#https://github.com/GoogleContainerTools/distroless/blob/main/examples/rust/Dockerfile
FROM gcr.io/distroless/cc-debian12:nonroot

WORKDIR /app

COPY default.config.toml ./
COPY --from=builder --chown=nonroot:nonroot /app/target/release/ism ./

USER nonroot

ENV RUST_LOG=info
ENV ISM_MODE=production

EXPOSE 5403

CMD ["./ism"]
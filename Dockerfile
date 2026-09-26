FROM rust:1.95-slim-bookworm AS builder
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config cmake \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations
COPY openapi.yaml ./openapi.yaml
RUN cargo build --release --locked --bin asystant_api

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl sqlite3 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 asystant \
    && useradd --uid 10001 --gid asystant --no-create-home asystant
COPY --from=builder /app/target/release/asystant_api /usr/local/bin/asystant_api
RUN mkdir -p /data && chown 10001:10001 /data && chmod 700 /data
VOLUME ["/data"]
USER 10001:10001
ENV ASYSTANT_BIND=0.0.0.0:8787 DATABASE_PATH=/data/asystant.db
EXPOSE 8787
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD curl --fail --silent http://127.0.0.1:8787/health/ready || exit 1
ENTRYPOINT ["asystant_api"]
CMD []

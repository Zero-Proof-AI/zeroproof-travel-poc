# Stage 1: Builder
FROM rust:1.83-bookworm AS builder

WORKDIR /app

RUN apt-get update && apt-get install -y \
    build-essential \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

COPY agent-a/Cargo.toml agent-a/Cargo.lock ./

COPY agent-a/mcp-server/ ./mcp-server/
COPY agent-a/mcp-client/ ./mcp-client/
COPY agent-a/src/ ./src/

COPY zk-protocol/ ../zk-protocol/

RUN cargo build --release --bin mcp-client-http -p mcp-client

# Debug: list binaries
RUN ls -la /app/target/release/

# Stage 2: Runtime
FROM debian:bookworm-slim

WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/mcp-client-http /app/mcp-client-http

EXPOSE 3001

HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:3001/health || exit 1

ENTRYPOINT ["/app/mcp-client-http"]
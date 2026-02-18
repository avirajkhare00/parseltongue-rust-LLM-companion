# Build stage
FROM rust:1.93.1 as builder
WORKDIR /usr/src/parseltongue
COPY . .
RUN apt-get update \
  && apt-get install -y build-essential clang libclang-dev pkg-config libssl-dev unzip ca-certificates \
  && rm -rf /var/lib/apt/lists/*

# Build all binaries (parseltongue, parseltongue-mcp, etc.)
RUN cargo build --release --bins

# Runtime image
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates unzip && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /usr/src/parseltongue/target/release/parseltongue /usr/local/bin/parseltongue
COPY --from=builder /usr/src/parseltongue/target/release/parseltongue-mcp /usr/local/bin/parseltongue-mcp

# Create data and uploads directories (mounted at runtime by platform)
RUN mkdir -p /data /uploads
EXPOSE 7777
ENTRYPOINT ["/usr/local/bin/parseltongue"]

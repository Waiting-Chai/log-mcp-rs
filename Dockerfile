# Build stage
FROM rust:1-slim-bookworm AS builder

# Change source to Aliyun
RUN sed -i 's/deb.debian.org/mirrors.aliyun.com/g' /etc/apt/sources.list.d/debian.sources

WORKDIR /usr/src/app
COPY . .

# Configure Cargo to use rsproxy mirror for faster builds in China
RUN mkdir -p .cargo && \
    echo '[source.crates-io]' > .cargo/config.toml && \
    echo 'replace-with = "rsproxy-sparse"' >> .cargo/config.toml && \
    echo '[source.rsproxy]' >> .cargo/config.toml && \
    echo 'registry = "https://rsproxy.cn/crates.io-index"' >> .cargo/config.toml && \
    echo '[source.rsproxy-sparse]' >> .cargo/config.toml && \
    echo 'registry = "sparse+https://rsproxy.cn/index/"' >> .cargo/config.toml && \
    echo '[registries.crates-io]' >> .cargo/config.toml && \
    echo 'protocol = "sparse"' >> .cargo/config.toml

# Install build dependencies
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

# Build the application
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

# Change source to Aliyun
RUN sed -i 's/deb.debian.org/mirrors.aliyun.com/g' /etc/apt/sources.list.d/debian.sources

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && apt-get install -y libssl-dev ca-certificates && rm -rf /var/lib/apt/lists/*

# Copy the binary from the builder stage
COPY --from=builder /usr/src/app/target/release/log-search-mcp /usr/local/bin/log-search-mcp

# Copy example config
COPY config.example.yaml /app/config.yaml

# Expose port (default 3000 as per example config)
EXPOSE 3000

# Set entrypoint
ENTRYPOINT ["log-search-mcp"]
CMD ["/app/config.yaml"]

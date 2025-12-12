# Build stage
FROM rust:1-slim-bookworm AS builder

# Change source to Aliyun
RUN sed -i 's/deb.debian.org/mirrors.aliyun.com/g' /etc/apt/sources.list.d/debian.sources

WORKDIR /usr/src/app
COPY . .

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

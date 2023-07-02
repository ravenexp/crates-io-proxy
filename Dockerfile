#
# Dockerfile for the crates-io-proxy server application
#

### First stage: Build the application itself.
FROM rust:alpine as builder

WORKDIR /builds/crates-io-proxy

# Copy source data (see .dockerignore for excludes).
COPY . .

# Install the build deps and build the application with cargo.
RUN \
apk add musl-dev && \
cargo build --release

### Second stage: Copy the built application into the runtime image.
FROM alpine:latest as runner

LABEL version="0.1.4"
LABEL description="crates.io proxy container image"
LABEL maintainer="Sergey Kvachonok <ravenexp@gmail.com>"

# Install the compiled executable into the system.
COPY --from=builder /builds/crates-io-proxy/target/release/crates-io-proxy /usr/bin/crates-io-proxy

# Add the proxy service user and create the crate files cache directory writable by it.
RUN \
adduser -SHD -u 777 -h /var/empty -s /sbin/nologin -g "crates.io proxy" cratesioxy && \
mkdir /var/cache/crates-io-proxy && \
chown cratesioxy /var/cache/crates-io-proxy

# Switch to the service user to run the proxy process.
USER cratesioxy
WORKDIR /var/empty

# Run the proxy server with the default configuration.
CMD crates-io-proxy --verbose

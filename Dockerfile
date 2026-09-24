# syntax=docker/dockerfile:1
#
# Two-stage build: compile the `nsm` binary on the Rust Alpine image, then copy
# it into a minimal Alpine runtime that runs as an unprivileged user.
#
#   docker build -t nsm .
#   docker run --rm -p 12000:12000 nsm
#
# The musl build uses the pure-Rust `ring` crypto provider so no cmake or C
# toolchain beyond musl-dev is needed.

ARG RUST_VERSION=1.98

FROM rust:${RUST_VERSION}-alpine AS build
WORKDIR /app
RUN apk add --no-cache musl-dev

# Dependencies are fetched from crates.io with Cargo.lock enforced (--locked);
# the in-repo vendor/ directory is not used inside the image.
RUN --mount=type=bind,source=src,target=src \
    --mount=type=bind,source=Cargo.toml,target=Cargo.toml \
    --mount=type=bind,source=Cargo.lock,target=Cargo.lock \
    --mount=type=cache,target=/app/target/ \
    --mount=type=cache,target=/usr/local/cargo/git/db \
    --mount=type=cache,target=/usr/local/cargo/registry/ \
    cargo build --locked --release --bin nsm --no-default-features --features ring && \
    cp ./target/release/nsm /bin/nsm

FROM alpine:3.22 AS final

# ca-certificates: platform trust store for TLS clients when no --root-ca is given.
RUN apk add --no-cache ca-certificates && \
    adduser --disabled-password --gecos "" --home /nonexistent \
            --shell /sbin/nologin --no-create-home --uid 10001 appuser
USER appuser

COPY --from=build /bin/nsm /usr/local/bin/nsm

# Broker port. The bind address is selected from the container's interface
# (eth0 on the default Docker network); override the CMD for other setups.
EXPOSE 12000

HEALTHCHECK --interval=30s --timeout=3s CMD ["/usr/local/bin/nsm", "--version"]

ENTRYPOINT ["/usr/local/bin/nsm"]
CMD ["listen", "--transport", "http", "--bind-port", "12000", "-n", "eth0", "--ip-version", "4"]

# TLS variant: mount cert/key, set CERT_PATH / KEY_PATH, and append "--tls".

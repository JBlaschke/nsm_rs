# syntax=docker/dockerfile:1
#
# Two-stage build: compile one NSM binary on the Rust Alpine image, then copy it
# into a minimal Alpine runtime that runs as an unprivileged user.
#
#   docker build --build-arg APP_NAME=api -t nsm .
#   docker run --rm -p 12000:12000 nsm
#
# APP_NAME selects the binary to ship: `api` (HTTP transport) or `tcp` (raw TCP).

ARG RUST_VERSION=1.98
ARG APP_NAME=api

FROM rust:${RUST_VERSION}-alpine AS build
ARG APP_NAME
WORKDIR /app

# clang/lld/musl-dev are needed for the aws-lc-sys crypto provider on musl.
RUN apk add --no-cache clang lld musl-dev git

# Dependencies are fetched from crates.io with Cargo.lock enforced (--locked);
# the in-repo vendor/ directory is not used inside the image.
RUN --mount=type=bind,source=src,target=src \
    --mount=type=bind,source=Cargo.toml,target=Cargo.toml \
    --mount=type=bind,source=Cargo.lock,target=Cargo.lock \
    --mount=type=cache,target=/app/target/ \
    --mount=type=cache,target=/usr/local/cargo/git/db \
    --mount=type=cache,target=/usr/local/cargo/registry/ \
    cargo build --locked --release --bin "$APP_NAME" && \
    cp "./target/release/$APP_NAME" /bin/nsm-server

FROM alpine:3.22 AS final

# ca-certificates: platform trust store for TLS clients when no --root_ca is given.
RUN apk add --no-cache ca-certificates && \
    adduser --disabled-password --gecos "" --home /nonexistent \
            --shell /sbin/nologin --no-create-home --uid 10001 appuser
USER appuser

COPY --from=build /bin/nsm-server /bin/nsm-server

# Broker port. The bind address is selected from the container's interface
# (eth0 on the default Docker network); override the CMD for other setups.
EXPOSE 12000

ENTRYPOINT ["/bin/nsm-server"]
CMD ["listen", "-n", "eth0", "--ip-version", "4", "--bind-port", "12000"]

# TLS variant: mount cert/key and set CERT_PATH / KEY_PATH, then run
#   CMD ["listen", "-n", "eth0", "--ip-version", "4", "--bind-port", "12000", "--tls"]

# NSM: an HPC-aware service connectivity tool

NSM (NERSC Service Mesh) is a connection broker for environments where compute
nodes cannot accept inbound connections from the outside. Services *publish*
themselves to a broker that has a fixed address; jobs *claim* a service by a
shared key and receive its address; the broker keeps both sides alive with
heartbeats and re-pairs clients when a service disappears.

> **Status: mid-cleanup.** See [`docs/PLAN.md`](docs/PLAN.md) for the plan and
> [`docs/audit/`](docs/audit/README.md) for the audit behind it. The command
> line below is final; the backend behind it is being rewritten, so protocol
> details (timeouts, wire format) will change in the next stage.

## Building

```bash
cargo build --release
```

Dependencies are vendored under `vendor/` and `.cargo/config.toml` points Cargo
at them, so a clean checkout builds without network access
(`cargo build --offline`). One binary is produced: `target/release/nsm`.

TLS uses [rustls](https://github.com/rustls/rustls) with the `aws-lc-rs` crypto
provider by default. For a static musl build, or on hosts without a C toolchain
and cmake, use the pure-Rust provider instead:

```bash
cargo build --release --no-default-features --features ring
```

## Usage

```
nsm [--log-level FILTER] <COMMAND>
```

| Command | Purpose |
|---|---|
| `nsm list-interfaces` | interfaces on this host |
| `nsm list-ips [-n IFACE] [-i PREFIX] [--ip-version 4\|6]` | addresses on this host |
| `nsm listen --bind-port PORT [--transport tcp\|http] [--tls]` | run the broker |
| `nsm publish BROKER --bind-port PORT --service-port PORT --key KEY` | register a service |
| `nsm claim BROKER --bind-port PORT --key KEY` | pair with a service |
| `nsm collect PARTY` | read what a party holds |
| `nsm send PARTY --msg TEXT` | send a message to a client for relay to its service |
| `nsm serve [--bind ADDR]` | REST control plane (loopback by default) |

`BROKER` and `PARTY` are addresses of the form `host:port` (raw TCP),
`http://host:port` or `https://host:port`; the transport follows from the
scheme. `listen` has no peer, so it takes `--transport`. The old snake_case
spellings `list_interfaces` and `list_ips` are accepted as aliases.

Every party needs one local address to advertise. When the host has several,
narrow the choice with `-n/--name IFACE`, `-i/--ip-start PREFIX` and
`--ip-version 4|6`; an ambiguous selection is reported with the candidates.

A minimal session on one machine (macOS loopback is `lo0`, Linux `lo`):

```bash
# Broker
nsm listen --bind-port 12000 -n lo0 --ip-version 4

# A service that listens on port 9000, with heartbeats on 12010
nsm publish 127.0.0.1:12000 --bind-port 12010 --service-port 9000 --key 1234 -n lo0 --ip-version 4

# A client asking for a service with the same key; heartbeats on 12020
nsm claim 127.0.0.1:12000 --bind-port 12020 --key 1234 -n lo0 --ip-version 4
```

`nsm <command> --help` lists every option.

## Environment

| Variable | Purpose |
|---|---|
| `NSM_LOG_LEVEL` | log filter: `trace`, `debug`, `info`, `warn` (default), `error`, or a `tracing` directive such as `nsm=debug` |
| `NSM_LOG_STYLE` | `auto` (default; colour only on a terminal), `always`, `never` |
| `CERT_PATH`, `KEY_PATH` | PEM certificate and private key for `--tls` |
| `ROOT_PATH` | PEM root CA bundle for verifying peers (same as `--root-ca`); platform store when unset |

Logs go to stderr; stdout carries only a command's result. A template is in
[`.env.example`](.env.example).

## Docker

```bash
docker compose up --build
```

runs a broker on port 12000 over HTTP; see [`compose.yaml`](compose.yaml) for
the TLS variant. Kubernetes RBAC notes are in [`deploy/k8s/`](deploy/k8s/README.md).

## Documentation

API documentation is built by CI with `cargo doc` and published to
<https://jblaschke.github.io/nsm_rs/> (the repository's Pages source must be set
to "GitHub Actions" for this to take effect).

# NSM: an HPC-aware service connectivity tool

NSM (NERSC Service Mesh) is a connection broker for environments where compute
nodes cannot accept inbound connections from the outside. Services *publish*
themselves to a broker that has a fixed address; jobs *claim* a service by a
shared key and receive its address; the broker keeps both sides alive with
heartbeats and re-pairs clients when a service disappears.

> **Status: mid-cleanup.** This repository is being restructured; see
> [`docs/PLAN.md`](docs/PLAN.md) for the plan and [`docs/audit/`](docs/audit/README.md)
> for the audit that motivates it. The description below is of the code as it is
> today, including its rough edges. The command line will change in the next
> stage (single `nsm` binary with subcommands).

## Building

```bash
cargo build --release
```

Dependencies are vendored under `vendor/` and `.cargo/config.toml` points Cargo
at them, so a clean checkout builds without network access
(`cargo build --offline`). Rust 1.83 or newer is required today.

Two binaries are produced:

| Binary | Transport between broker and parties |
|---|---|
| `target/release/tcp` | raw TCP, JSON messages |
| `target/release/api` | HTTP(S), JSON bodies (`--tls` enables TLS on the server side) |

Both accept the same command line.

## Usage

```
<binary> <OPERATION> [HOST] [OPTIONS]
```

Operations: `list_interfaces`, `list_ips`, `listen`, `publish`, `claim`,
`collect`, `send`. `HOST` is the broker or party to talk to, written as
`host:port`, `http://host:port` or `https://host:port`.

A minimal session on one machine (replace `en0` with your interface and pick an
IPv4 address with `--ip-version 4`):

```bash
# Broker
./target/release/tcp listen -n en0 --ip-version 4 --bind-port 12000

# A service announcing that it listens on port 9000, with heartbeats on 12010
./target/release/tcp publish 10.0.0.5:12000 -n en0 --ip-version 4 \
    --bind-port 12010 --service-port 9000 --key 1234

# A client asking for a service with the same key; heartbeats on 12020
./target/release/tcp claim 10.0.0.5:12000 -n en0 --ip-version 4 \
    --bind-port 12020 --key 1234
```

Common options:

| Option | Meaning |
|---|---|
| `-n, --name <NAME>` | interface to take the local address from |
| `-i, --ip-start <OCTETS>` | pick the local address whose text starts with these octets |
| `--ip-version <4\|6>` | address family |
| `--bind-port <PORT>` | port this party listens on for heartbeats |
| `--service-port <PORT>` | (publish) port the actual service listens on |
| `--key <KEY>` | rendezvous key shared by a service and its clients |
| `--msg <MSG>` | (send) message to deliver |
| `--tls` | (api) serve TLS; needs `CERT_PATH` and `KEY_PATH` |
| `--root_ca <PEM>` | root CA bundle for verifying the peer |
| `--ping` | one-sided heartbeats from the party to the broker |
| `-v, --verbose` | print section headers in `list_*` output |

## Environment

| Variable | Purpose |
|---|---|
| `NSM_LOG_LEVEL` | `trace`, `debug`, `info`, `warn` (default), `error` |
| `NSM_LOG_STYLE` | `auto`, `always` (default), `never` |
| `CERT_PATH`, `KEY_PATH` | PEM certificate and private key for `--tls` |
| `ROOT_PATH` | PEM root CA bundle for TLS clients (platform store when unset) |

A template is in [`.env.example`](.env.example).

## Docker

```bash
docker compose up --build
```

builds the `api` binary and runs a broker on port 12000; see
[`compose.yaml`](compose.yaml) for the TLS variant. Kubernetes RBAC notes are in
[`deploy/k8s/`](deploy/k8s/README.md).

## Documentation

API documentation is built by CI with `cargo doc` and published to
<https://jblaschke.github.io/nsm_rs/> (the repository's Pages source must be set
to "GitHub Actions" for this to take effect).

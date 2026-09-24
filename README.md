# NSM: an HPC-aware service connectivity tool

NSM (NERSC Service Mesh) is a connection broker for environments where compute
nodes cannot accept inbound connections from the outside. Services *publish*
themselves to a broker that has a fixed address; jobs *claim* a service by a
shared key and receive its address; the broker keeps both sides alive with
heartbeats and re-pairs clients when a service disappears.

> **Status: mid-cleanup.** See [`docs/PLAN.md`](docs/PLAN.md) for the plan and
> [`docs/audit/`](docs/audit/README.md) for the audit behind it. The backend was
> rewritten in the `cleanup/03-common-backend` branch: one implementation
> serves raw TCP, TCP+TLS, HTTP and HTTPS. The wire format changed with it;
> parties and brokers must run the same version.

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

## How it works

```
                 publish (key K)                     claim (key K)
  service  ───────────────────────►  broker  ◄───────────────────────  client
  :9000    ◄──── heartbeats ────────  :12000  ──────── heartbeats ────►  
  hb :12010                                                             hb :12020
                                        ▲
        client ◄─────────────── "here is service K: host:9000" (claim reply)
        client ◄── send TEXT ── operator     text is relayed via the broker and
        service ◄── TEXT on the next heartbeat;   `collect` reads it back
```

- Every party runs a small listener on its **bind port** (heartbeat endpoint).
  The broker dials it periodically (two-sided heartbeats); with `--ping` the
  party pings the broker instead (one-sided, for parties behind NAT).
- A service is claimed exclusively by one client until that client goes away.
  If a service disappears, its clients are re-paired with another service that
  published under the same key, and learn the new address on their next
  heartbeat; if there is none, they are removed.
- A party that stops hearing from the broker exits with an error (exit code 1);
  nothing exits from inside a handler.

## Usage

```
nsm [--log-level FILTER] <COMMAND>
```

| Command | Purpose |
|---|---|
| `nsm list-interfaces [--ip-version 4\|6]` | interfaces on this host |
| `nsm list-ips [-n IFACE] [-i PREFIX] [--ip-version 4\|6]` | addresses on this host |
| `nsm listen --bind-port PORT [--transport tcp\|tls\|http\|https]` | run the broker |
| `nsm publish BROKER --bind-port PORT --service-port PORT --key KEY [--ping]` | register a service |
| `nsm claim BROKER --bind-port PORT --key KEY [--ping]` | pair with a service; prints `host:port` |
| `nsm collect PARTY` | a service's last received text, or a client's service address |
| `nsm send PARTY --msg TEXT` | hand text to a client for delivery to its service |
| `nsm serve [--bind ADDR] [--token TOKEN]` | REST control plane (loopback by default) |

`BROKER` and `PARTY` are addresses of the form `host:port` (raw TCP),
`tls://host:port`, `http://host:port` or `https://host:port`; the transport
follows from the scheme, and a party's own listener uses the same family as
its broker. `listen` has no peer, so it takes `--transport`. The old
snake_case spellings `list_interfaces` and `list_ips` are accepted as aliases.

Every party needs one local address to advertise. When the host has several,
narrow the choice with `-n/--name IFACE`, `-i/--ip-start PREFIX` and
`--ip-version 4|6`; an ambiguous selection is reported with the candidates.

A minimal session on one machine (macOS loopback is `lo0`, Linux `lo`):

```bash
nsm listen --bind-port 12000 -n lo0 --ip-version 4
nsm publish 127.0.0.1:12000 --bind-port 12010 --service-port 9000 --key 1234 -n lo0 --ip-version 4
nsm claim   127.0.0.1:12000 --bind-port 12020 --key 1234 -n lo0 --ip-version 4   # prints 127.0.0.1:9000
nsm send    127.0.0.1:12020 --msg "job 17"
nsm collect 127.0.0.1:12010                                                       # prints: job 17
```

`nsm <command> --help` lists every option.

### TLS

| Flag / variable | Meaning |
|---|---|
| `--tls-cert PEM` (`CERT_PATH`), `--tls-key PEM` (`KEY_PATH`) | identity this process presents when it serves TLS |
| `--tls` | serve TLS on a party's own listener (needs the two above) |
| `--root-ca PEM` (`ROOT_PATH`) | CA bundle used to verify peers; the platform trust store when unset |

A broker started with `--transport tls` or `https` needs a certificate and
key. Clients verify the broker against `--root-ca`. Trust anchors never travel
over the wire, and a connection configured for TLS never falls back to
plaintext. Mutual TLS (authenticating parties to the broker) is not implemented
yet; see the plan.

### REST control plane

`nsm serve` exposes the same operations over HTTP for orchestrators:
`GET /healthz`, `GET /v1/interfaces`, `GET /v1/ips`, `POST /v1/publish` and
`POST /v1/claim` (both return `202` with a job), `GET /v1/jobs`,
`GET|DELETE /v1/jobs/{id}`, `POST /v1/collect`, `POST /v1/send`. It binds
`127.0.0.1:8080` by default; binding any other address requires `--token`
(sent as `Authorization: Bearer`). Request bodies never carry file paths.
Full field lists are in the `nsm::rest` module documentation.

## Environment

| Variable | Purpose |
|---|---|
| `NSM_LOG_LEVEL` | log filter: `trace`, `debug`, `info`, `warn` (default), `error`, or a `tracing` directive such as `nsm=debug` |
| `NSM_LOG_STYLE` | `auto` (default; colour only on a terminal), `always`, `never` |
| `CERT_PATH`, `KEY_PATH`, `ROOT_PATH` | defaults for `--tls-cert`, `--tls-key`, `--root-ca` |
| `NSM_TOKEN` | default for `nsm serve --token` |

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

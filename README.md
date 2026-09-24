# NSM: an HPC-aware service connectivity tool

NSM (NERSC Service Mesh) is a connection broker for environments where compute
nodes cannot accept inbound connections from the outside. Services *publish*
themselves to a broker that has a fixed address; jobs *claim* a service by a
shared key and receive its address; the broker keeps both sides alive with
heartbeats and re-pairs clients when a service disappears.

One binary, `nsm`, is the broker, the service side, the client side and a REST
control plane. It speaks four transports (raw TCP, TCP+TLS, HTTP, HTTPS) with
one protocol.

> **Status: mid-cleanup.** The backend was rewritten in 2026-09 (see
> [`docs/PLAN.md`](docs/PLAN.md) and [the audit](docs/audit/README.md)). The
> wire format, the binary name and several defaults changed; parties and
> brokers must run the same version. [`CHANGELOG.md`](./CHANGELOG.md) lists
> every breaking change.

## Contents

- [Building](#building)
- [How it works](#how-it-works)
- [Quickstart](#quickstart)
- [Command-line reference](#command-line-reference)
- [TLS](#tls)
- [REST control plane](#rest-control-plane)
- [Logging](#logging)
- [Deployment](#deployment)
- [Notes for HPC systems](#notes-for-hpc-systems)
- [Testing](#testing)
- [Documentation](#documentation)

## Building

```bash
cargo build --release
```

Dependencies are vendored under `vendor/` and `.cargo/config.toml` points Cargo
at them, so a clean checkout builds without network access
(`cargo build --offline`). One binary is produced: `target/release/nsm`. The
minimum supported Rust is 1.88 (`rust-version` in `Cargo.toml`, edition 2024);
CI builds and tests on it as well as on stable.

TLS uses [rustls](https://github.com/rustls/rustls) with the `aws-lc-rs` crypto
provider by default, which needs a C compiler (and cmake on some targets). For
a static musl build, or on hosts without a C toolchain, use the pure-Rust
provider instead; exactly one provider must be enabled:

```bash
cargo build --release --no-default-features --features ring
```

```bash
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl --no-default-features --features ring
```

## How it works

```text
                 publish (key K)                     claim (key K)
  service  ───────────────────────►  broker  ◄───────────────────────  client
  :9000    ◄──── heartbeats ────────  :12000  ──────── heartbeats ────►
  hb :12010                                                             hb :12020
                                        ▲
        client ◄─────────────── "here is service K: host:9000" (claim reply)
        client ◄── send TEXT ── operator     text is relayed via the broker and
        service ◄── TEXT on the next heartbeat;   `collect` reads it back
```

- The **broker** (`nsm listen`) is the only party with a fixed, well-known
  address. It keeps a registry of services and clients and never carries the
  service's data traffic: a client connects to the service directly, using the
  address the broker handed it.
- A **service** (`nsm publish`) registers under a rendezvous **key** together
  with the port its real service listens on. A **client** (`nsm claim`) asks
  for a service under the same key and gets one service's `host:port`,
  exclusively, until the client goes away.
- Every party runs a small listener on its **bind port** (the heartbeat
  endpoint). The broker dials it every heartbeat interval (two-sided
  heartbeats); with `--ping` the party pings the broker instead (one-sided,
  for parties behind NAT). A party that stops answering is removed after a
  configurable number of failures; a party that stops hearing from its broker
  exits with an error.
- If a service disappears, its client is re-paired with another service that
  published under the same key, and learns the new address on its next
  heartbeat; if there is none, the client is removed and its process exits.
- `nsm send` hands a short text to a client; the client relays it through the
  broker and the paired service receives it on its next heartbeat. `nsm
  collect` reads a service's last received text, or a client's paired service
  address.
- Every registration reply carries a **registration token** (128 random bits)
  known only to the broker and that party. Pings, relayed messages and the
  broker's heartbeats must present it, so a peer that knows an id or the
  rendezvous key cannot inject text, re-pair a party or keep a dead one alive.
  The token never appears in a service handle, a job view or a log.

[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) describes the code,
[`docs/PROTOCOL.md`](docs/PROTOCOL.md) the messages and the timing rules.

## Quickstart

A complete session on one machine. Every party needs one local address to
advertise; `-i 127.` selects the loopback address on Linux and macOS alike
(see [address selection](#address-selection)).

```bash
nsm listen --bind-port 12000 -i 127. --ip-version 4
```

```bash
nsm publish 127.0.0.1:12000 --bind-port 12010 --service-port 9000 --key 1234 -i 127. --ip-version 4
```

```bash
nsm claim 127.0.0.1:12000 --bind-port 12020 --key 1234 -i 127. --ip-version 4
# prints: 127.0.0.1:9000
```

```bash
nsm send 127.0.0.1:12020 --msg "job 17"
nsm collect 127.0.0.1:12010        # prints: job 17     (after the next heartbeat)
nsm collect 127.0.0.1:12020        # prints: 127.0.0.1:9000
```

`publish` and `claim` keep running: they are the party. Stop them with Ctrl-C
(or SIGTERM), which unregisters nothing but stops answering heartbeats, and
the broker removes the party after the failure threshold. The same session
over HTTP: start the broker with `--transport http` and give the parties
`http://127.0.0.1:12000`.

## Command-line reference

```text
nsm [--log-level FILTER] <COMMAND>
```

| Command | Purpose | Prints on stdout |
|---|---|---|
| `nsm list-interfaces [--ip-version 4\|6] [-v]` | interfaces on this host | one name per line |
| `nsm list-ips [-n IFACE] [-i PREFIX] [--ip-version 4\|6] [-v]` | addresses on this host | one address per line |
| `nsm listen --bind-port PORT [--transport tcp\|tls\|http\|https] [options]` | run the broker | nothing |
| `nsm publish BROKER --bind-port PORT --service-port PORT --key KEY [--ping] [options]` | register a service and keep it registered | nothing |
| `nsm claim BROKER --bind-port PORT --key KEY [--ping] [options]` | pair with a service and stay paired | the service's `host:port` |
| `nsm collect PARTY [options]` | a service's last received text, or a client's service address | the text or the `host:port` |
| `nsm send PARTY --msg TEXT [options]` | hand text to a client for delivery to its service | nothing |
| `nsm serve [--bind ADDR] [--token TOKEN] [options]` | REST control plane | nothing |

`nsm <command> --help` lists every option with its default. The snake_case
spellings `list_interfaces` and `list_ips` are accepted as aliases. `collect`
and `send` accept a hidden `--key` for compatibility with old scripts; it is
ignored.

**Addresses.** `BROKER` and `PARTY` are `host:port` (raw TCP),
`tls://host:port`, `http://host:port` or `https://host:port`; the transport
follows from the scheme, and a party's own listener uses the same family as
its broker (TCP for `host:port` and `tls://`, HTTP for `http://` and
`https://`). IPv6 literals are written in brackets: `[fe80::1]:12000`. `listen`
has no peer, so it takes `--transport`. `--bind-port 0` picks a free port; the
broker and the parties print the address they actually bound on stderr.

**Exit codes and output.** 0 on success; 1 when an operation fails at run time
(a message prefixed `nsm: ` goes to stderr); 2 for a command-line error. Stdout
carries only a command's result, so it can be captured by scripts; logs and
status lines go to stderr.

### Address selection

Every party advertises exactly one local address to the broker. When the host
has several, narrow the choice; an ambiguous selection is refused with the
candidates listed.

| Option | Meaning |
|---|---|
| `-n, --name IFACE` | take the address from this interface (`eth0`, `hsn0`, `lo0`) |
| `-i, --ip-start PREFIX` | only addresses whose text starts with `PREFIX` (`10.128.`, `127.`) |
| `--ip-version 4\|6` | only this address family (both are considered when omitted) |

### TLS options

| Option | Environment | Meaning |
|---|---|---|
| `--tls-cert PEM` | `CERT_PATH` | certificate chain this process presents when it serves TLS |
| `--tls-key PEM` | `KEY_PATH` | private key matching `--tls-cert` |
| `--root-ca PEM` | `ROOT_PATH` | CA bundle used to verify peers (`--root_ca` is accepted as an alias) |
| `--system-roots` | | trust the platform certificate store instead of `--root-ca` (off by default) |
| `--tls` | | serve TLS on a party's own listener; needs the certificate and key |

See [TLS](#tls) for which side needs what.

### Timing options

Seconds, fractions allowed (`--heartbeat-interval 0.5`). Available on every
command that talks to peers; the defaults live in the `nsm::config` module.

| Option | Default | Meaning |
|---|---|---|
| `--heartbeat-interval SECS` | 2 | time between heartbeats (broker to party) or pings (party to broker) |
| `--heartbeat-timeout SECS` | 3 | how long one heartbeat may take before it counts as failed |
| `--fail-threshold N` | 5 | consecutive failed heartbeats after which the broker removes a party |
| `--ping-staleness SECS` | 20 | silence after which a pinging party is removed |
| `--broker-watchdog SECS` | 30 | silence after which a party gives up on its broker and exits |
| `--request-timeout SECS` | 6 | deadline for one request/response exchange |
| `--connect-timeout SECS` | 5 | deadline for connecting, including the TLS handshake |

With the defaults a silent two-sided party is removed after about 25 seconds
(five heartbeats of up to three seconds, two seconds apart). Registration is
retried six times one second apart, and a claim waits 1.5 seconds for a
service to appear before it is refused; these two are not overridable.

### Limit options

On `listen` and `serve`.

| Option | Default | Meaning |
|---|---|---|
| `--max-frame-bytes BYTES` | 65536 | largest message accepted on any transport (at least 1024) |
| `--max-connections N` | 1024 | connections a listener serves at once; more wait in the accept queue |
| `--max-registrations N` | 10000 | services plus clients a broker holds at once |

### Admission options

On `listen`.

| Option | Default | Meaning |
|---|---|---|
| `--max-registrations-per-host N` | 64 | registrations accepted per advertised host |
| `--require-matching-host` | off | refuse a party whose advertised address is not the one it connected from |

`--require-matching-host` stops a peer from pointing the broker's heartbeats at
a third party; leave it off when parties sit behind NAT or advertise a
different interface on purpose.

## TLS

Which side needs what:

| Process | Needs |
|---|---|
| broker with `--transport tls` or `https` | `--tls-cert` and `--tls-key`; `--root-ca` to dial parties that serve TLS |
| party (`publish`, `claim`) talking to a TLS broker | `--root-ca` (or `--system-roots`) to verify the broker; `--tls-cert`/`--tls-key` if it serves TLS itself |
| `collect`, `send` against a party that serves TLS | `--root-ca` (or `--system-roots`) |
| `serve` | the same material, handed to the parties it starts |

A party serves TLS on its own listener when asked with `--tls`, or
automatically when its broker speaks TLS and a certificate and key are
configured. The broker then dials the party's heartbeat endpoint over TLS and
verifies it against `--root-ca`.

Trust anchors are operator configuration. An internal mesh should name its own
CA with `--root-ca` rather than accept every public one; `--system-roots` is
an explicit opt-in, and a host with no platform store at all (common on
compute nodes and minimal images) reports a configuration error rather than
silently trusting nothing. Trust anchors never travel over the wire, and a
connection configured for TLS never falls back to plaintext. The TLS
configuration is checked before anything is dialled, so a missing trust root
is reported as such.

Certificates must name the host the peers dial (`127.0.0.1` and `localhost`
for local tests; the node's DNS name or IP on a cluster). TLS 1.2 and 1.3 are
accepted; HTTP transports negotiate `http/1.1` through ALPN. Mutual TLS
(authenticating parties to the broker by certificate) is not implemented yet;
see the plan.

## REST control plane

`nsm serve` exposes the same operations over HTTP for orchestrators
(a Kubernetes sidecar, a workflow engine):

| Route | Purpose |
|---|---|
| `GET /healthz` | liveness |
| `GET /v1/interfaces`, `GET /v1/ips` | local addresses, with the same filters as the CLI |
| `POST /v1/publish`, `POST /v1/claim` | start a party as a background *job*; `202` with the job |
| `GET /v1/jobs`, `GET /v1/jobs/{id}`, `DELETE /v1/jobs/{id}` | list, inspect and stop jobs |
| `POST /v1/collect`, `POST /v1/send` | the `collect` and `send` operations |

It binds `127.0.0.1:8080` by default. Binding any other address requires
`--token` (or `NSM_TOKEN`), which clients send as `Authorization: Bearer
<token>` on every request, including `/healthz`. Request bodies never carry
file paths: TLS material comes from the `serve` process's own flags. Bodies
are limited to 64 KiB. Every route, field and status code is documented in
[`docs/REST_API.md`](docs/REST_API.md).

## Logging

| Variable | Purpose |
|---|---|
| `NSM_LOG_LEVEL` | log filter: `trace`, `debug`, `info`, `warn` (default), `error`, or a `tracing` directive such as `nsm=debug` |
| `NSM_LOG_STYLE` | `auto` (default; colour only on a terminal), `always`, `never` |
| `--log-level FILTER` | the same as `NSM_LOG_LEVEL`, taking precedence |

Logs go to stderr; stdout carries only a command's result. Message payloads and
tokens are never logged. Other environment variables: `CERT_PATH`, `KEY_PATH`,
`ROOT_PATH` (defaults for the TLS flags) and `NSM_TOKEN` (default for `nsm
serve --token`). A template is in [`.env.example`](.env.example).

## Deployment

**Docker.** The [`Dockerfile`](./Dockerfile) builds a static musl binary with
the `ring` provider and runs it as an unprivileged user on Alpine:

```bash
docker build -t nsm .
docker run --rm -p 12000:12000 nsm
```

The default command runs a broker over HTTP on port 12000, advertising the
container's `eth0` address; override the command for other setups. For TLS,
mount the certificate and key, set `CERT_PATH`/`KEY_PATH` and append `--tls`.

**Compose.** `docker compose up --build` runs the same broker; see
[`compose.yaml`](./compose.yaml) for the TLS variant.

**Kubernetes.** The broker is a plain Deployment with one Service on its bind
port; parties inside the cluster reach it by DNS name, parties outside through
whatever ingress exposes that port (TCP, not HTTP-only, unless the broker runs
`--transport http`). The RBAC notes and the one committed manifest are in
[`deploy/k8s/`](deploy/k8s/README.md). Run `nsm serve` as a sidecar when a
controller should start parties over HTTP.

## Notes for HPC systems

- **Static binary.** Build with `--no-default-features --features ring` for a
  musl target; the result has no dynamic dependencies and runs on compute
  nodes with a minimal image. Nothing needs a C toolchain or cmake with the
  `ring` provider.
- **Offline builds.** `vendor/` and `.cargo/config.toml` make every build
  offline; no registry access is needed on the login or build node.
- **Interface selection.** Nodes usually have several interfaces (management,
  high-speed network, loopback). Select the one the other parties can reach
  with `-n hsn0` or `-i 10.128.`, and check with `nsm list-ips -v`.
- **Firewalls and NAT.** The broker's bind port must be reachable from every
  party. Two-sided heartbeats additionally need the parties' bind ports
  reachable from the broker; when they are not (parties behind NAT, or a
  broker outside the cluster), run the parties with `--ping` so all traffic
  flows from the party to the broker.
- **Timing.** The defaults detect a dead party in about 25 seconds. Batch
  jobs with long scheduler pauses may need a larger `--fail-threshold` or
  `--heartbeat-timeout`; use the same values on the broker and its parties.
- **Job scripts.** `nsm claim` prints the service address once paired and
  then keeps running; start it in the background, read its first stdout
  line, and stop it when the job ends. It exits 1 when the broker is lost or
  no replacement service exists.

## Testing

```bash
cargo test --offline                                 # unit, integration and doc tests
cargo test --offline --test stress -- --ignored      # 50 services and 50 clients on one broker
cargo deny --all-features check                      # advisories, licenses, sources, duplicates
cargo llvm-cov --offline --summary-only              # line coverage (CI enforces a floor)
```

Unit tests live next to the code; the randomized address and framing tests
draw from a seeded generator in `src/testing.rs`, so a failure names the
iteration that produced it. Under `tests/`, `e2e.rs` runs a broker with
services and clients over all four transports, `rest.rs` exercises every
control-plane route, `cli.rs` drives the built binary through complete
sessions, and `stress.rs` is the load test. CI runs all of this on Linux and
macOS with both crypto providers, plus rustfmt, clippy, rustdoc, cargo-deny,
cargo-machete, a Docker build and a coverage floor (`.github/workflows/ci.yml`).
[`CONTRIBUTING.md`](./CONTRIBUTING.md) has the details.

## Documentation

| Document | Contents |
|---|---|
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | crate layout, components, concurrency and error rules |
| [`docs/PROTOCOL.md`](docs/PROTOCOL.md) | wire format, every message, sequence diagrams, timing and re-pairing rules |
| [`docs/REST_API.md`](docs/REST_API.md) | the control plane's routes, bodies and status codes |
| [`CONTRIBUTING.md`](./CONTRIBUTING.md) | building, testing, dependency and protocol changes |
| [`CHANGELOG.md`](./CHANGELOG.md) | what changed, including every breaking change |
| [`docs/PLAN.md`](docs/PLAN.md), [`docs/audit/`](docs/audit/README.md) | the cleanup plan and the audit behind it |

API documentation (`cargo doc`) and these pages are published by CI to
<https://jblaschke.github.io/nsm_rs/> (the repository's Pages source must be
set to "GitHub Actions" for the deploy step to take effect). Tagging `vX.Y.Z`
runs the release workflow: static musl binaries (`ring`), glibc and macOS
binaries (`aws-lc-rs`), a vendored source tarball for air-gapped builds, and
the container image on GHCR. No license has been chosen for this repository
yet; see the plan.

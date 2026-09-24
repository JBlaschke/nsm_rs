# Changelog

All notable changes to NSM are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

The 2026-09 cleanup rewrote the backend once for every transport. Everything
below compares against the last pre-cleanup state of `main`. Wire protocol
version: 1 (the first versioned format).

### Breaking changes

- **One binary.** `nsm` with subcommands (`listen`, `publish`, `claim`,
  `collect`, `send`, `list-interfaces`, `list-ips`, `serve`) replaces the
  `tcp` and `api` binaries and their positional `OPERATION HOST` arguments.
  The snake_case spellings `list_interfaces` and `list_ips` remain as aliases.
  Decision D2.
- **Wire format.** Messages are a tagged JSON object (`"type"`) carried in
  4-byte length-prefixed frames on TCP and TLS, or as the body of
  `POST /v1/message` on HTTP and HTTPS. Registrations return the party's id
  and a registration token; every reply has one shape. Pre-cleanup brokers
  and parties cannot talk to this version. Decision D3.
- **Transport from the address.** `host:port` is raw TCP, `tls://`, `http://`
  and `https://` select the others; `listen` takes `--transport`. A party's
  own listener uses the same family as its broker.
- **TLS flags.** `--tls-cert`/`--tls-key` (environment `CERT_PATH`/`KEY_PATH`)
  name the identity; `--root-ca` (`--root_ca` still accepted, `ROOT_PATH`)
  names the trust anchors; the platform trust store is only used with the new
  `--system-roots`. Trust anchors no longer travel in the registration
  payload, and a TLS connection never falls back to plaintext. Decision D10.
- **Claims are exclusive** until the client is removed. The 60-second lease
  that re-issued a live service to a second client is gone. Decision D7.
- **Failure detection** actually applies the threshold: a party is removed
  after 5 consecutive failed heartbeats (2 s interval, 3 s timeout, about
  25 s in total) instead of the first one. A party that stops hearing its
  broker exits with code 1 instead of calling `process::exit(0)` from a
  request handler. Decision D8.
- **Control plane.** `nsm serve` binds `127.0.0.1:8080` and requires
  `--token` (or `NSM_TOKEN`) for any other address; `publish` and `claim`
  return `202` with a job that `GET`/`DELETE /v1/jobs/{id}` inspects and
  stops; request bodies never carry file paths; routes live under `/v1/`.
  Decision D9.
- **Service handles** (the claim reply, `collect` on a client, REST job views)
  no longer contain the rendezvous key.
- `collect` and `send` ignore `--key` (still accepted, hidden).
- **Logging** uses `tracing`; `NSM_LOG_LEVEL` and `NSM_LOG_STYLE` keep their
  meaning, `--log-level` overrides them, logs go to stderr and stdout carries
  only a command's result.

### Added

- A license: the BSD 3-Clause License (`LICENSE`, `license` field in
  `Cargo.toml`). The repository had none before.
- A library crate (`nsm`) with the binary as a thin front-end; every operation
  is a typed function in `nsm::ops`.
- Four transports from one implementation: TCP, TCP+TLS (`tls://`, new),
  HTTP, HTTPS.
- Registration tokens: 128-bit secrets issued at registration and required on
  pings, relayed messages and the broker's heartbeats.
- Broker admission policy: `--max-registrations-per-host` (default 64) and
  `--require-matching-host`.
- Flags for every timing and limit: `--heartbeat-interval`,
  `--heartbeat-timeout`, `--fail-threshold`, `--ping-staleness`,
  `--broker-watchdog`, `--request-timeout`, `--connect-timeout`,
  `--max-frame-bytes`, `--max-connections`, `--max-registrations`.
- `--bind-port 0` picks a free port; the bound address is printed on stderr.
- Graceful shutdown on Ctrl-C and SIGTERM.
- Tests: 160+ unit tests, end-to-end tests over all four transports, control
  plane and binary tests, a stress test; CI on Linux and macOS with both
  crypto providers, plus rustdoc, clippy, cargo-deny, cargo-machete, a Docker
  build and a coverage floor.
- Documentation: README with the full command-line reference,
  `docs/ARCHITECTURE.md`, `docs/PROTOCOL.md`, `docs/REST_API.md`,
  `CONTRIBUTING.md`, this changelog, rustdoc on every public item, and a
  Pages workflow publishing all of it.
- Crypto provider features that work: `aws-lc-rs` (default) and `ring` (pure
  Rust, for static musl builds and the Docker image). Decision D11.

### Changed

- Every dependency is at its latest version (2026-09-24): among the direct
  ones `clap` 4.6 and `rcgen` 0.14; in the lock file `bytes` 1.12, `ring`
  0.17.14, `hashbrown` 0.17, the ICU crates 2.3 and about ninety more. Only
  `matchit` stays at the version `axum` pins. The yanked `spin` is gone.
- Edition 2024 and `rust-version = "1.88"`, the minimum the latest
  dependencies need; CI builds and tests on that toolchain as well as on
  stable. Decision D12.
- `tokio` and `hyper-util` are enabled with only the features the code uses
  instead of `full`.
- Release automation: tagging `vX.Y.Z` builds static musl binaries (`ring`)
  for x86_64 and aarch64, glibc and macOS binaries (`aws-lc-rs`), a vendored
  source tarball for air-gapped builds and the container image on GHCR, and
  publishes a GitHub release with the changelog section as notes. Dependabot
  watches the GitHub Actions and Cargo dependencies weekly.
- Interface enumeration uses `if-addrs` instead of `pnet` (35 crates fewer).
  Decision D6.
- HTTP servers are axum, HTTP clients are reqwest, both on hyper 1.x with
  rustls. Decision D4.
- Parties bind their listener before registering, so the broker can dial them
  the moment registration succeeds.
- Re-paired clients learn their new service in the next heartbeat instead of
  silently keeping a stale address.
- `Addr` stores IP literals in canonical form, so `::1` and `0::1` are the
  same host.
- The Dockerfile builds a static musl binary with `ring` and runs as an
  unprivileged user; the compose file and Kubernetes notes were updated.

### Removed

- The legacy `tcp`/`api` code paths, the `ComType` argument threaded through
  every function and the `(Option<TcpStream>, Option<Request>)` pairs.
- Generated rustdoc, `target*/` directories, the Docker tarball and the
  committed `.env` from the repository (they remain in history; see
  `docs/PLAN.md` section 2).
- Direct dependencies `pnet`, `hyper-rustls`, `lazy_static`, `base64`, `url`
  and friends that the new backend does not need.

### Fixed

- `send` never delivered (the relayed message always named party 0).
- HTTP two-sided heartbeats panicked the broker's monitor task.
- The REST control plane was unreachable because argument parsing exited first.
- A single missed heartbeat evicted a party.
- An empty or malformed TCP connection panicked the broker's accept loop.
- An idle connection to any party's bind port made that party exit.
- Heartbeats of many parties were serialised through one 200 ms tick, so
  the period grew with the number of parties.
- `collect` on a party over TLS without a trust root reported "connection
  refused" instead of the missing configuration.
- The control plane reported a 500 for an unreachable broker or party; it now
  reports 502 as documented.

### Security

- Ping, relay and heartbeat messages are authorised by registration tokens;
  refusals for unknown ids and wrong tokens are indistinguishable.
- Rendezvous keys no longer leak through service handles or job views.
- The HTTP client follows no redirects.
- Trust anchors are operator configuration; the platform store is opt-in.
- The control plane is loopback-only without a token, and never reads
  server-side files named by a request.
- Frame sizes, body sizes, connection counts and registrations are bounded;
  release builds keep integer overflow checks.
- `unwrap`, `expect` and `panic!` are denied outside tests.
- The dependency update closes RUSTSEC-2026-0007 (`bytes` 1.9.0,
  `BytesMut::reserve` overflow) and RUSTSEC-2025-0009 (`ring` 0.17.8, AES
  panic with overflow checks); `cargo audit` and `cargo deny` report nothing
  open, and yanked crates now fail the check.
- Known open items: the TLS key committed in 2025 (`01a90972`) must be
  rotated, and mutual TLS is not implemented.

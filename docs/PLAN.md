# NSM cleanup plan

Written 2026-09-24 against `main` at `edd23a33`. Companion document: [the audit](audit/README.md), which holds every finding this plan responds to.

## 0. Status

| Branch | State | Notes |
|---|---|---|
| `cleanup/01-repo-hygiene` | done | 5 commits; tracked files 23,449 → 11,914 |
| `cleanup/02-foundation` | done | single `nsm` binary, lib crate, typed protocol, rustls module, 77 unit tests; legacy code runs under `src/legacy/` |
| `cleanup/03-common-backend` | done | backend written once (transport trait; TCP, TLS, HTTP, HTTPS), registry actor with per-party monitors, party sessions, typed ops, REST control plane with jobs; legacy deleted; 129 unit + 11 end-to-end tests over all four transports |
| `cleanup/04-hardening` | done | platform trust store opt-in (`--system-roots`), broker admission policy (matching-host check, per-host registration cap), CLI overrides for every timing and limit, `deny(unwrap_used, expect_used, panic)` outside tests, release overflow checks; security review of the branch found 4 items (unauthenticated `Ping`/`Deliver`, forgeable party heartbeats, the rendezvous key inside `ServiceHandle` and job views), all fixed with per-registration tokens |
| `cleanup/05-tests-ci` | next | |
| `cleanup/06-docs` | planned | |
| `cleanup/07-deps` | planned | |

Commits inside a branch group changes by topic for reading; only the branch tip is guaranteed to build. Vendor updates are always their own commit (`chore: re-vendor`) so they can be skipped in review.

## 1. Why this plan exists

NSM started as a TCP-only broker. A second, HTTP-based mode was added alongside it, and the two were never merged into one backend. Today the same seven operations exist in two hand-written variants each, selected partly by which binary you run (`tcp` or `api`), partly by a `ComType` argument threaded through every function, and partly by `(Option<TcpStream>, Option<Request>)` pairs that panic on the combinations nobody intended. The audit found 216 issues across seven lenses; the headline ones are:

- **Neither mode works end to end.** `send` never delivers (the relayed message id is always 0), HTTP two-sided heartbeats are a stub that makes the broker's monitor task panic, the REST control plane is unreachable because clap exits before it can start, and a single missed heartbeat evicts a party even though the code was written for a ten-failure threshold.
- **The broker is trivially killable.** One empty or malformed TCP connection panics the accept loop; an idle connection to any party's bind port makes that party call `process::exit`.
- **There is no test, no CI, no lib target,** so none of the above could have been caught, and nothing can be refactored safely until that changes.
- **The repository carries 530 MB of generated or vendored content** and, in its history, a private TLS key.

The way out is not to tidy the two copies but to write the backend once, against a transport abstraction, and delete both copies. Everything else in this plan either prepares for that or builds on it.

## 2. Things that need your action, independent of the code

These are outside what a branch can fix.

1. **Rotate the TLS key.** `server.key` and `server.csr` were committed in `01a90972` (2025-01-03) and deleted in `ca85e236`, but the blobs remain reachable from `origin/main`, `origin/jpb/sync` and `origin/jpb/sync1`. Treat the certificate that key backs as compromised.
2. **Decide on a history rewrite.** The pack is 330 MB, dominated by `target 2/` and `target 3/` debug binaries (20 to 32 MB each), the Docker tarball, `docs/` and `vendor/`. A one-time `git filter-repo` pass would shrink it to single-digit megabytes and remove the key, but it changes every commit hash and requires collaborators to re-clone. This plan does not perform the rewrite; branch 01 removes the files from the tip so a rewrite later is a pure history operation.
3. **Switch GitHub Pages to "GitHub Actions".** Pages currently serves the stale rustdoc from `main:/docs` in legacy mode. Branch 01 deletes `docs/` (generated HTML) and adds a Pages workflow; the docs site will be empty until the repository setting is flipped (Settings, Pages, Source).
4. **Choose a license.** The repository is public with no `LICENSE` file and no `license` field. This is a legal call, so the plan leaves it out; the Cargo metadata and README are written so a license can be dropped in.
5. **Delete stale remote branches** once you have confirmed nothing on them is wanted: `condvar` (one broken experiment), `sofia_nsm_rs` (one commit deleting a script), and the fully merged `rest_api`, `jpb/code_cleanup`, `jpb/sync`, `jpb/sync1`, `merge`.

## 3. Decisions taken in this plan

Each decision below was made so the work could proceed without blocking. The recommended default is what the branches implement; the "if you disagree" note says what changes.

| # | Decision | Default taken | If you disagree |
|---|---|---|---|
| D1 | **Vendoring.** `vendor/` (356 MB, 205 crates, ~60 of them Windows/Android-only) is tracked and `.cargo/config.toml` forces it for every build. | Keep the current workflow through the cleanup: every branch that changes `Cargo.lock` re-runs `cargo vendor` in its own commit. Branch 07 adds a release job that produces a platform-filtered vendor tarball, so dropping `vendor/` from git later is a one-line change. | Say so and branch 01 removes `vendor/` and the source replacement instead; offline HPC builds then use the release tarball or a warmed `CARGO_HOME`. |
| D2 | **One binary.** `tcp` and `api` re-declare the same module tree; transport is chosen by which binary runs. | A single `nsm` binary with subcommands (`listen`, `publish`, `claim`, `collect`, `send`, `list-interfaces`, `list-ips`, `serve`). Transport comes from the address scheme (`host:port` is TCP, `http://`, `https://`), plus `--transport` on `listen`/`serve` which have no peer address. | Keep two thin binary names as aliases; the library is identical either way. |
| D3 | **Wire format may change.** The current framing (a read shorter than 1024 bytes ends a message) and the JSON-in-a-string envelope cannot be made safe without changing bytes on the wire. | Length-prefixed frames on TCP, a tagged `Message` enum, server-assigned ids in every reply, one response shape per request. No compatibility with the current `tcp`/`api` binaries; all parties upgrade together. | If old and new binaries must interoperate, a compatibility shim is a separate branch after 03. |
| D4 | **HTTP stack.** Seven hand-rolled hyper accept loops and fourteen connector builders. | `axum` for every HTTP server (broker transport, party heartbeat endpoint, REST control plane) and `reqwest` with rustls for every HTTP client. Both sit on hyper 1.x, so nothing moves off the current runtime. | Staying on raw hyper is possible with one shared `http.rs`; it keeps the legacy client and hand-rolled routing. |
| D5 | **Logging.** `log` + `env_logger`, with `println!` used for both data output and state dumps. | `tracing` + `tracing-subscriber`; `NSM_LOG_LEVEL` and `NSM_LOG_STYLE` keep working. Stdout carries only the operation's result. | Bump `log`/`env_logger` instead; the stdout discipline still applies. |
| D6 | **Interface enumeration.** `pnet` pulls 35 crates (and 114 MB of Windows-only vendored code) for one `interfaces()` call. | `if-addrs`. | Keep `pnet` 0.35; only `network.rs` differs. |
| D7 | **Claim semantics.** Today a claim is a 60-second lease that re-issues a live service to a second client, and client records are themselves claimable after 60 s. | A service is claimed exclusively by one client until that client is removed by the broker. No time-based lease. Services and clients are separate record types. | A lease can be re-added as an optional `--claim-lease` on the broker. |
| D8 | **Failure detection.** The code intends ten failures, throttled to one per 5 s; the implementation removes on the first failure. | Per-party heartbeat task; a party is removed after `fail_threshold` consecutive failed heartbeats (default 5, with a 2 s interval and 3 s timeout, so roughly 10 to 25 s to detect death). All timings live in one `Timing` struct, overridable from the CLI. | Any defaults you prefer; they are configuration, not code. |
| D9 | **Control plane exposure.** The REST front-end binds `0.0.0.0:8080` with no authentication and lets a caller start arbitrary long-lived tasks and read server-side PEM files. | `nsm serve` binds `127.0.0.1` by default, requires a bearer token when bound elsewhere, returns `202` with a job id for long-running operations, exposes `GET /v1/jobs/{id}`, and never accepts file paths in request bodies. | Any of these can be loosened by flag; the defaults are the safe direction. |
| D10 | **Trust anchors.** Today each party ships its own CA bundle to the broker inside the registration payload, so the party being verified chooses the roots that verify it. | Trust anchors are operator configuration (`--root-ca` on whichever side dials out). `root_ca` leaves the wire protocol. Peer authentication (mTLS) is scoped as a follow-up, not in these branches. | Keep payload-carried CA as an explicit opt-in flag if a deployment depends on it. |
| D11 | **Crypto provider.** `[features] ring = []` and `aws-lc-rs = []` forward nothing; the build works only because `hyper-rustls` defaults pull `aws-lc-rs`, whose C build needs cmake. | Real features: `aws-lc-rs` default for gnu targets, `ring` for static musl release builds. Exactly one provider installed once in `main`. | `ring` everywhere is the simplest choice for HPC; it is a one-line default change. |
| D12 | **Edition and MSRV.** Edition 2021, no `rust-version`; the Dockerfile pins Rust 1.83, below the 1.85 the latest `clap`/`reqwest`/`hyper-rustls` need. | Edition 2024, `rust-version = "1.85"`, Dockerfile on the current stable image. | Stay on 2021 and pin older dependency versions in branch 07. |

## 4. Target architecture

```
src/
  lib.rs            crate docs, module tree, lint policy (deny let_underscore_future, unused_must_use; warn unwrap_used, missing_docs)
  main.rs           nsm binary: install crypto provider, init tracing, parse CLI, run, map Result to exit code
  cli.rs            clap derive: Cli { global opts } + Command::{ListInterfaces, ListIps, Listen, Publish, Claim, Collect, Send, Serve}
  error.rs          thiserror Error enum + crate Result; one axum IntoResponse mapping
  config.rs         Timing (all intervals/timeouts/thresholds), TlsPaths, Limits (frame size, body size, max connections)
  net/
    addr.rs         Addr { transport: Transport, host, port: u16 } FromStr/Display, socket_addr(), url(path); IPv6 literals
    interfaces.rs   local IP enumeration + name / prefix / version filters (if-addrs)
  protocol/
    message.rs      #[serde(tag = "type")] enum Message { Publish, Claim, Ack, Nack, Heartbeat, Ping, Deliver, Collect, Collected }
    codec.rs        JSON encode/decode with size cap; length-delimited framing for TCP
    types.rs        Key, PartyId, ServiceRecord, ClientRecord, ServiceHandle (what a claimer receives), MsgBody
  transport/
    mod.rs          trait Client { async fn call(&self, addr: &Addr, msg: Message) -> Result<Message> }
                    trait Handler { async fn handle(&self, msg: Message, peer: PeerInfo) -> Result<Message> }
                    async fn serve(addr, tls: Option<ServerTls>, handler: Arc<dyn Handler>, shutdown) -> Result<BoundServer>
    tcp.rs          TcpClient / tcp server: one framed request-response per connection, optional TLS via tokio-rustls, timeouts, semaphore
    http.rs         HttpClient (reqwest) / axum server: POST /v1/message; same Handler; optional TLS
    tls.rs          ServerConfig / ClientConfig from TlsPaths (PEM via rustls-pki-types), https-only client when TLS configured
  broker/
    registry.rs     Registry: services and clients by key and id, claim/release/remove/reclaim, pending message per service. Pure, sync, unit-tested.
    monitor.rs      one heartbeat task per party (two-sided) or staleness check (ping); reports outcomes over a channel to the registry owner
    handler.rs      BrokerHandler: Publish -> Ack(id); Claim -> Ack(ServiceHandle) | Nack; Ping -> mark alive; Deliver -> store message
    mod.rs          listen(): bind transport server, run registry actor + monitor, graceful shutdown
  party/
    handler.rs      PartyHandler (role Publisher | Claimer): Heartbeat -> reply + store inbox; Collect -> Collected(inbox | service handle); Deliver (from `send`) -> relay to broker
    publisher.rs    publish(): register, run party server on bind addr, watchdog (returns Err(BrokerLost), never exits the process), optional ping loop
    claimer.rs      claim(): request, print/return ServiceHandle, same party server
  ops/              list_interfaces, list_ips, listen, publish, claim, collect, send: written once, return typed results, no printing
  rest/             axum control plane for `nsm serve`: typed request structs shared with the CLI, job registry, bearer token, loopback default
tests/
  common/           spawn broker/publisher/claimer in-process on 127.0.0.1:0 with Timing::fast(); rcgen certs for TLS
  tcp_e2e.rs, http_e2e.rs, tls_e2e.rs, reclaim.rs, send_collect.rs, rest.rs, cli.rs
```

Import direction is strictly downward: `main -> cli -> rest/ops -> broker/party -> transport -> protocol/net -> error/config`. Nothing below `ops` prints, panics on peer input, or exits the process.

### Semantics preserved from today

- Key-based rendezvous: services publish under a `key`; a claimer with the same key receives one service's address and port.
- The broker is the only fixed address; it dials each party's bind address for two-sided heartbeats, or accepts one-sided pings (`--ping`).
- Server-assigned monotonically increasing ids; a client remembers the id of the service it claimed.
- A dead service's clients are re-pointed to another live service under the same key if one exists; otherwise they are removed. A client's removal frees its service.
- `send` to a client's bind address is relayed through the broker to the paired service and delivered on the next heartbeat; `collect` on a service returns the last delivered message; `collect` on a client returns its service's handle.
- Operation surface, address grammar (`host:port`, `http://host:port`, `https://host:port`), `--ping`, `--tls`, `--root-ca`, `NSM_LOG_LEVEL`.

### Semantics deliberately changed

- Framing, envelope, ids in replies (D3). Claim exclusivity (D7). Failure threshold actually applied (D8). Re-claimed clients are told their new service in the next heartbeat instead of silently keeping a stale address. A party that loses its broker returns an error and exits non-zero from `main` instead of `process::exit(0)` from a handler. Trust anchors are configured, not received (D10). TLS is available on the TCP transport too, which today has none.

## 5. Branches

Branches are stacked: each is based on the previous one, so PR *n+1* reviews as a diff against PR *n*. Names are `cleanup/NN-topic`. Each branch must build, pass `cargo test`, and leave the binary usable end to end (except where noted) before the next one starts. Nothing is pushed or merged without your say-so.

### 01 `cleanup/01-repo-hygiene` (this branch)

Behaviour-neutral. Makes the repository reviewable and gives later branches a CI gate.

- Add `docs/PLAN.md` (this file) and `docs/audit/`.
- Remove from the tip: `nsm-dev-buildx-latest.tar`, `src/.DS_Store`, `.env` (replaced by `.env.example`), the generated rustdoc under `docs/`, `src/test_event_monitor.sh` (targets a binary and CLI syntax that no longer exist), `README.Docker.md` (folded into `README.md`).
- Rewrite `.gitignore` (fix the `server.keyl` and `.yam` typos; ignore `.env`, `.DS_Store`, key material, archives, `target*/`).
- Move `view-events-rolebinding.yaml` to `deploy/k8s/` with a note about the ignored `Role` it pairs with.
- De-template `Dockerfile` and `compose.yaml`; make the Docker `CMD` a command the current CLI accepts (TLS opt-in via mounted certs, no macOS interface name); current stable Rust image, current Alpine.
- Interim `README.md` that documents what actually exists today (two binaries, positional operation, env vars) and links the plan.
- Cargo metadata (`description`, `repository`, `readme`); remove the two dead dependencies (`threadpool`, `rustls-platform-verifier`) since no code references them (re-vendor).
- `.github/workflows/ci.yml`: build, test, clippy (warnings allowed until 02), rustdoc. `.github/workflows/pages.yml`: `cargo doc` to Pages.
- Acceptance: `cargo build --offline` and `cargo test --offline` pass; `git ls-files | wc -l` drops from 23,449 to roughly 12,500 (vendor remains, D1).

### 02 `cleanup/02-foundation`

Additive scaffolding plus a mechanical move; behaviour unchanged, old code still runs.

- `src/lib.rs` and a single `nsm` binary (D2) with clap-derive subcommands. Old operations are called through the new CLI; `tcp.rs`, `api.rs`, `cli.rs` are deleted. The unreachable REST front-end becomes `nsm serve` (still the old handlers, now actually reachable).
- New modules with unit tests: `error`, `config` (`Timing`, `TlsPaths`, `Limits`), `net::addr` (fixes IPv6 literals, port range, trailing slash), `net::interfaces` (if-addrs, D6), `protocol` (typed `Message`, codec, length-delimited framing).
- `tracing` initialised in `main` (D5); the old code's `log` macros keep working through the bridge.
- Real crypto-provider features (D11), installed once in `main`.
- Old `service.rs`, `operations.rs`, `mode_*`, `connection.rs`, `api_builder.rs`, `models.rs`, `tls.rs`, `network.rs`, `utils.rs` move under `src/legacy/` with only path fixes, so branch 03's diff is a deletion of `legacy/` plus additions.
- Formatting and the ~120 mechanical clippy lints are applied to the new tree only; CI switches to `cargo fmt --check` and `clippy -D warnings` scoped to non-legacy code.
- Acceptance: `nsm listen/publish/claim` over TCP behaves as `tcp` did; all new modules have tests; `cargo doc` clean.

### 03 `cleanup/03-common-backend`

The core of the work: the backend written once, both copies deleted.

- `transport` (TCP with optional TLS, HTTP via axum/reqwest, D4), `broker` (registry actor, per-party monitor, handler), `party` (publisher/claimer handler, watchdog, ping), `ops` (once), `rest` (typed control plane with job registry, D9).
- Fixes by construction: the `send` id bug, the stubbed HTTP heartbeat handler, first-failure eviction, the lost re-claim on a cloned `State`, lock held across sleeps and connects, `only_or_error` on peer data, every `unwrap` on peer input, every `process::exit`.
- Tests: registry and monitor unit tests under paused time; end-to-end tests over TCP, HTTP and HTTPS (rcgen) covering publish/claim/heartbeat/send/collect/service death and re-claim/broker loss.
- Delete `src/legacy/`. Re-vendor. Apply the security `cargo update` (rustls 0.23.45, h2, tokio, webpki, aws-lc) here since the lock changes anyway.
- Acceptance: all e2e tests green on both transports; `grep -c 'unwrap()' src/` outside tests near zero; no `process::exit` outside `main.rs`.

### 04 `cleanup/04-hardening`

Behaviour-changing safety limits and the remaining audit items.

- Limits from `config::Limits`: max frame/body size, max in-flight connections (semaphore), TLS handshake and per-request timeouts, accept-error backoff.
- TLS: `https_only` when TLS is configured; no implicit native-roots fallback; trust anchors from configuration only (D10); ALPN `http/1.1` only.
- Control plane: bearer token, loopback default, exact-match routes, POST for body-carrying requests, no file paths from the network (D9).
- Input validation at the edges (`u16` ports, `u64` keys via `as_u64`, JSON booleans as booleans), overflow-checks in release, redaction of payloads and state from logs.
- Graceful shutdown on SIGINT/SIGTERM through a cancellation token.
- Lint policy: `deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)` outside tests.
- Acceptance: a security review of the diff (the `/security-review` skill) reports no open critical or high item from the audit's security lens.
- Outcome of that review (2026-09-24): four findings, all fixed on the branch. The broker now issues a 128-bit registration token (`protocol::RegToken`) in every `Registered`/`Paired` reply; `Ping` and `Deliver` must present it (`Deliver` also names the sending client, which must be paired with the target), the broker's `Heartbeat` carries it and parties ignore heartbeats without it (services also ignore pairings), `ServiceHandle` no longer contains the rendezvous key, and the HTTP client follows no redirects. Refusals for "unknown id" and "wrong token" share one text. Wire format changed again (still within D3).

### 05 `cleanup/05-tests-ci`

- Property tests for `Addr` and framing; table tests for the monitor's decisions; the 50-party stress test behind `#[ignore]`; `assert_cmd` CLI tests; malformed-input tests for both transports.
- CI: fmt, clippy `-D warnings` for both provider features, tests on Linux and macOS at stable and MSRV, `cargo doc -D warnings`, `cargo deny` (advisories, licenses, sources, duplicate versions), `cargo machete`, Docker build smoke test, coverage with `cargo llvm-cov` and a floor that ratchets up.
- Acceptance: CI green; coverage floor set from the measured value.

### 06 `cleanup/06-docs`

- `README.md`: purpose, architecture diagram, quickstart, CLI reference, REST reference, TLS setup, logging, deployment (Docker, compose, k8s), HPC notes (static binary, interface selection, offline builds).
- `docs/ARCHITECTURE.md`, `docs/PROTOCOL.md` (message schemas, sequence diagrams, timings, failure and re-claim rules), `docs/REST_API.md`, `CONTRIBUTING.md`, `CHANGELOG.md` (with the breaking changes of D2/D3 listed).
- Rustdoc on every public item, `#![deny(missing_docs)]`, README included as crate docs; Pages workflow publishes `cargo doc` plus the markdown docs.
- Acceptance: `cargo doc -D warnings` clean; every CLI flag and REST route documented.

### 07 `cleanup/07-deps`

The dependency bump you asked for, last, with tests in place to catch regressions.

- Bump every dependency to its latest stable (`cargo upgrade --incompatible`), edition 2024 and `rust-version = "1.85"` (D12), narrow `tokio`/`hyper-util` features, `rustls-pemfile` replaced by `rustls-pki-types` PEM parsing, `base64` and `url` dropped as direct deps, `lazy_static` gone.
- `deny.toml`, `.github/dependabot.yml`, `release.yml` (static musl binaries with `ring`, gnu binaries with `aws-lc-rs`, filtered vendor tarball for air-gapped builds, image to GHCR), Dockerfile on the current Rust image.
- Re-vendor (D1).
- Acceptance: `cargo deny check` and `cargo audit` clean; all tests green on MSRV and stable.

## 6. How to review

- Read `docs/audit/README.md` first for the shape of the problems, then this file's section 3 for the decisions.
- Review branches in order; each PR description will list the audit finding ids it closes (for example `S1`, `P3`, `C11`).
- The largest diffs are the re-vendor commits; they are isolated as `chore: re-vendor dependencies` so they can be skipped.

## 7. Out of scope (follow-ups after 07)

- Peer authentication (mTLS or per-key secrets) and authorisation of `publish`/`claim` by identity. The audit rates the missing authentication critical; this plan removes the ways it can crash the broker and stops trust anchors travelling in the payload, but does not add identity.
- Message queueing semantics beyond "last message wins" for `send`/`collect`.
- Kubernetes deployment manifests beyond the moved RoleBinding.
- The git history rewrite and remote branch deletion (section 2).

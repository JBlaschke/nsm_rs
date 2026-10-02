# Architecture

This document describes how the `nsm` crate is put together: the roles, the
modules, the rules every module follows, and why. For the wire format see
[`PROTOCOL.md`](PROTOCOL.md); for the control plane see
[`REST_API.md`](REST_API.md); the reasoning behind the current design is in
section 11 (Design decisions), and the record of the 2026 refactor that produced
it, plan and audit, is under [`history/2026-refactor/`](history/2026-refactor/PLAN.md).

## 1. Roles

```text
             ┌───────────────────────── broker (nsm listen) ─────────────────────────┐
             │  transport listener → BrokerHandler → Registry (pure state,           │
             │                        one store per claim)                           │
             │                        Broker monitor: heartbeat task per party,      │
             │                        sweeper for ping-mode parties                  │
             │                        Admin listener: /metrics, /v1/status           │
             └───────────▲──────────────────────────────────────────────▲────────────┘
     publish / ping /    │                                              │  claim / ping /
     deliver /           │                                              │  deliver /
     store_relay /       │                                              │  store_relay /
     heartbeats          │                                              │  heartbeats
   ┌─────────────────────┴──────────┐                     ┌─────────────┴──────────────────┐
   │ service party (nsm publish)    │                     │ client party (nsm claim)       │
   │ Session + PartyHandler         │ ◄── data traffic ── │ Session + PartyHandler         │
   │ listener on the bind port      │   (not NSM's job)   │ listener on the bind port      │
   └─────────────────────▲──────────┘                     └─────────────▲──────────────────┘
                         │ send / collect /                             │ send / collect /
                         │ store                                        │ peer / store
                    operator, script, or `nsm serve` (REST control plane) driving `ops`
```

- The **broker** is the only fixed address. It admits registrations, pairs
  clients with services, monitors liveness, relays short texts and keeps the
  one copy of the store each claim shares; it also answers an operator who
  knows only a rendezvous key (`store_by_key`, behind `nsm store --key`),
  resolving the key to its one claim. It never sees the service's own
  traffic. With `--admin-bind` it also answers an operator, on a separate
  plain-HTTP socket parties never use: Prometheus metrics and a status
  document ([`MONITORING.md`](MONITORING.md)).
- A **party** is a service or a client. Both run the same small server on
  their bind address, keep the last text they were sent, relay `send` to
  their peer through the broker and relay `store` to the broker; a client
  also keeps the service it is paired with. A party keeps nothing of the
  store.
- **Operations** (`ops`) are the verbs, written once, without printing. The
  CLI (`main`) and the REST control plane (`rest`) are two front-ends for
  them.

## 2. Crate layout

One library crate, `nsm`, and one binary of the same name built from
`src/main.rs`.

| Module | Role |
|---|---|
| `cli` | clap definitions; the same structs are the REST request bodies, so both front-ends share one set of options and validation |
| `config` | `Timing`, `Limits`, `BrokerPolicy`, `TlsPaths`: every interval, timeout, limit and file location, with documented defaults |
| `error` | the crate `Error` and `Result`; nothing below `main` panics on peer input or exits the process |
| `net` | `Addr` (transport, host, port; text form on the wire) and local interface enumeration with the `--name`/`--ip-start`/`--ip-version` filters |
| `protocol` | the tagged `Message` enum, the records it carries, JSON encoding and the length-prefixed `MessageCodec` for streams |
| `tls` | rustls server and client configuration from PEM files; the trust model in one place; crypto provider selection |
| `transport` | one request, one reply over TCP, TLS, HTTP or HTTPS behind the `Handler` trait, `serve()` and `Client::call()` |
| `broker` | `Registry` (pure, synchronous state), `Store` (the key-value store each claim shares with its service), `Broker` (monitor tasks and removal), `BrokerHandler` (admission and dispatch), `Metrics` and `Status` (what the broker counts), the admin listener that reports them, `listen()` |
| `party` | `Session` (bind, register, stay alive), `PartyHandler`, `PartyState` |
| `ops` | typed requests and results for every operation |
| `rest` | the axum control plane behind `nsm serve`: routes, jobs, bearer token |
| `logging` | `tracing` initialisation honouring `NSM_LOG_LEVEL` and `NSM_LOG_STYLE` |
| `testing` (test builds only) | a seeded generator for the randomized tests |

Import direction is strictly downward:

```text
main → cli → rest / ops → broker / party → transport → protocol / net / tls → config / error
```

## 3. Rules every module follows

1. **Nothing below `main` prints, panics on peer input, or exits the
   process.** Results are returned; `main` prints them to stdout and maps
   errors to exit codes. The lint policy enforces the panic part:
   `clippy::unwrap_used`, `expect_used`, `panic`, `unreachable`, `todo` and
   `unimplemented` are denied outside tests, `unused_must_use` and
   `let_underscore_future` always, and every public item has documentation
   (`deny(missing_docs)`).
2. **No lock is held across an `.await`.** The broker's registry sits behind
   a `std::sync::Mutex`; every access is a synchronous closure
   (`Broker::with_registry`) that returns before anything asynchronous
   happens. Party state uses the same pattern.
3. **Every task is owned and cancellable.** Long-running work runs under a
   `CancellationToken` hierarchy rooted in `main`'s signal handler: the
   listener, each accepted connection (tracked in a `JoinSet` and aborted on
   shutdown), each heartbeat task and the sweeper. Ctrl-C or SIGTERM cancels
   the root; nothing is left running mid-heartbeat.
4. **Every exchange is one request and one reply,** bounded by
   `Timing::request_timeout`, and every message by `Limits::max_frame_bytes`.
   Malformed input is answered by the transport (closed connection or HTTP
   400) and never reaches a handler; anything a handler does not expect is a
   `Nack`.
5. **Peer-controlled integers never wrap silently.** Release builds keep
   `overflow-checks = true`.
6. **Secrets stay out of logs.** Registration tokens have a redacted `Debug`,
   and message payloads are never logged.

## 4. Components

### Transport (`transport`)

```rust
pub trait Handler: Send + Sync + 'static {
    fn handle(&self, msg: Message, peer: PeerInfo) -> impl Future<Output = Result<Message>> + Send;
}
pub async fn serve<H: Handler>(bind: &Addr, handler: Arc<H>, tls: &TlsPaths, limits: &Limits, timing: &Timing, shutdown: CancellationToken) -> Result<Server>;
impl Client { pub async fn call(&self, to: &Addr, msg: Message) -> Result<Message>; }
```

The transport is chosen by `Addr::transport`, which comes from the address
scheme. `tcp.rs` accepts connections, wraps them in TLS when the listener is
`tls://`, and runs one framed request/reply exchange per connection under a
semaphore (`max_connections`) and a per-request deadline. `http.rs` mounts an
axum router (`POST /v1/message`, `GET /healthz`) on a manual hyper accept loop
so that the peer address is known, TLS is optional and every connection is
tracked; the client side is `reqwest` with rustls, HTTP/1.1 only, no
redirects, and `https_only` when the address is `https://`. Both sides share
`protocol::codec` for the body.

### Protocol (`protocol`)

`Message` is a `#[serde(tag = "type")]` enum: one variant per request and
reply. `codec::MessageCodec` is the tokio-util `Encoder`/`Decoder` for the
4-byte length prefix used on streams; `encode`/`decode` are the JSON functions
both transports use. `types` holds `PartyId`, `Key`, `RegToken` (a 128-bit
secret with constant-time comparison and a redacted `Debug`), `ServiceHandle`
(what a client is told about its service), the broker's records, and the
shared store's `StoreKey` (validated on parse and on decode), `StoreOp`,
`StoreEntry` and `Stored` (the reply to every store operation).

### Broker (`broker`)

- `registry.rs` is the model: services, clients, who holds whom, what is
  pending for whom, and one store per claim. It is synchronous, does no I/O,
  never reads the clock (callers pass `now`), and is exhaustively
  unit-tested. Ids come from one counter that never repeats, and store
  versions from another. A claim's store lives in the client's entry:
  `claim` creates it empty, `reclaim` leaves it in place so the replacement
  service reads every earlier write, and removing the client drops it, so
  `remove`, `reclaim` and `drop_party` need no store code at all.
  `Registry::store` finds the store the way `deliver` finds the peer: a
  client uses its own, also while orphaned; a service uses the store of the
  client holding it, and a service nobody holds reads an empty store and may
  not write. Store keys starting with `nsm_` are reserved for the broker:
  `Registry::store` answers them from `Registry::mesh_data` (where the
  claim's parties listen, projected from the records: one JSON value under
  `nsm_mesh_data` and one entry per field that is set, all at version 0),
  adds them to every `list`, refuses a write of any of them and never
  stores one.
  `Registry::resolve_key` is the one party a rendezvous key means (its one
  client, else its one service, or the party a request names, which must
  be under the key; anything else is refused with the candidates), and
  `Registry::store_by_key` applies an operation there.
- `store.rs` is `Store`: a key-value map with a byte budget, pure like the
  registry. Each entry counts as its JSON-encoded key and value plus 64
  bytes, which bounds the largest `stored` reply as well as memory; write
  numbers come from the registry's counter, passed in; a put or a delete
  may carry an `if_version`, compared first, and one that does not match is
  an answer with `applied` false that changes nothing; `Debug` shows counts
  only.
- `monitor.rs` is `Broker`: the registry behind its mutex, one heartbeat task
  per two-sided party (`watch`), a sweeper task for ping-mode parties, and
  the single removal path `drop_party`, which also re-pairs or removes the
  clients a vanished service leaves behind and counts what it did. It owns
  the `Metrics` too: the monitor counts heartbeats with their round trip,
  removals and re-pairings. `snapshot()` exposes the state for tests;
  `gauges()`, `render_metrics()` and `status()` expose it for monitoring.
- `metrics.rs` is what the broker counts and how it reports it (decisions
  D21 and D22). `Metrics` holds atomic counters, bumped where
  the event happens, and one histogram of heartbeat round trips; `Gauges::of`
  reads the current counts (parties by role and mode, unclaimed services,
  failing parties, stores and their bytes, with a per-key and a per-host
  breakdown) from the registry under its lock, so a gauge can never drift.
  `Metrics::render` writes the Prometheus text exposition by hand (format
  0.0.4, every metric `nsm_*`, labels from closed sets, no key, host or id
  as a label); `Status` is the JSON view the admin route and `nsm status`
  share. `RemovalReason` is the typed reason `drop_party` takes.
- `handler.rs` is `BrokerHandler`: admission (a real port, the optional
  matching-host check, the per-host cap) and one `match` over the request
  variants, written once for every transport. A relayed request (`deliver`,
  `store_relay`) checks the sender's token and acts on the registry in one
  critical section; a `store_by_key` carries no token, and the registry
  resolves its rendezvous key to one party in that same critical section. Every request is counted once by kind and outcome, and
  registrations, refusals (by reason) and store operations (by operation
  and outcome) where the decision is made.
- `admin.rs` is the admin listener (decision D23): an axum
  router with `GET /metrics` (the exposition, as
  `text/plain; version=0.0.4`), `GET /v1/status` (`Status` as JSON) and
  `GET /healthz`, on its own `TcpListener` under the broker's shutdown
  token, in two steps: `bind` (the loopback-or-token check, then the
  socket) and `AdminListener::serve`. It follows D9: loopback needs no
  token, any other bind address needs `--admin-token`, checked on every
  route in constant time (the control plane shares the comparison). It
  reads the broker and changes nothing.
- `listen.rs` wires the three together with a transport listener, after
  checking that a full store's reply fits the frame limit and, when an admin
  listener is wanted, binding its address first, so that a refused or taken
  admin address fails the start before any task runs; the admin listener
  is served once the protocol listener's address, which `/v1/status`
  reports, is known.

### Party (`party`)

`Session::publish` and `Session::claim` bind the party's own listener first
(so the broker can dial it the moment registration succeeds; on the port
`--bind-port` names, or on one the operating system picks when the flag is
left out, which the registration then carries), then register with retries,
then return a `Session` whose `run()` keeps the party alive:
in two-sided mode a watchdog that fails with `Error::BrokerLost` when the
broker's heartbeats stop for `broker_watchdog`; in ping mode a loop that
pings every `heartbeat_interval`, applies what the broker returns, and gives
up after `fail_threshold` failures or when the broker no longer knows the
party. `PartyHandler` answers the broker's `Heartbeat` (only with the party's
own token), `Collect`, `Send` (relayed to the broker as `Deliver`) and
`Store` (relayed to the broker as `StoreRelay`, whose `Stored` reply goes
back to the caller unchanged). Both relays go through one helper that
refuses before registration and adds the party's id and token, so the two
cannot drift apart; either role relays either request, and the broker
decides who the peer is and whose store it is. `PartyState` is the shared
state: id, token, inbox, paired service, last contact; nothing of the store
lives at a party. The pairing sits in a `tokio::sync::watch` channel:
`Session::pairings` hands out receivers, which is how `nsm claim` prints
every re-pairing as one more stdout line instead of keeping the new address
to itself.

### Operations and front-ends (`ops`, `cli`, `main`, `rest`)

`ops` turns typed requests into typed results. `main` parses the CLI, installs
the crypto provider and the signal handler, runs one operation, prints its
result to stdout and maps `Err` to exit code 1 with an `nsm: ` message on
stderr (clap's own usage errors exit 2; a party that answered but has nothing
to report yet, for `collect`, `peer` and `store get` of a key that is not
set, is exit 3, and a `store put` or `store delete` is exit 4 when a
condition was not met: its `--if-version` did not match, which is an answer,
not an `Error`). Every stdout line goes through one writer that reports a
closed pipe as an error, so a reader that went away is exit 1, not a panic;
`claim` exits 1 only when its first line cannot be written, and after that a
failed re-pairing line stops the printing but not the party.
`peer` and `collect` are the two accessors of `ops::Collected`, one per role;
the binary adds no logic of its own. `ops::store` takes a `StoreOp` to either
party and returns the broker's `Stored` as it is (a key that is not set is an
answer with no entry, a write whose condition was not met an answer with
`applied` false, a refusal is `Error::Rejected`); `ops::store_by_key` does
the same at the broker by rendezvous key, and `ops::StoreTarget` (a party's
address, or the broker's with the key and an optional party id) is the one
way in for both front-ends, so the command line and the control plane need
no store logic of their own: `nsm store` gets the target and the `StoreOp`
from `StoreCommand::into_parts` (`--key RENDEZVOUS` and `--party-id ID`
make the address the broker's) and only prints (the value, the new version,
the keys, or with `--json` the reply as one line), and `POST /v1/store`
returns the reply as its body, with status 409 and an `error` field when
`applied` is false, its body naming either `party` or `broker` and
`rendezvous`. `rest` serves
the same operations over HTTP: `publish` and `claim` become background jobs
with a view the API reports, cancels and reaps; a bearer token guards every
route when one is configured, and it is mandatory off loopback.
`ops::status` is the one operation that talks to a broker's admin listener
instead of a party: it reads `GET /v1/status` into `Status`, and `nsm
status` prints `Status::summary` (or the document as one JSON line with
`--json`), so the binary adds no formatting of its own. The control plane
has no status route: the admin listener already speaks HTTP.

## 5. Configuration

All tunables live in `config`: `Timing` (heartbeat interval and timeout,
failure threshold, ping staleness, broker watchdog, request and connect
timeouts, registration retries, claim wait), `Limits` (frame size,
concurrent connections, registrations, store budget), `BrokerPolicy`
(matching-host check, per-host cap) and `TlsPaths`. Defaults are documented
on the types and overridable from the CLI (`TimingOpts`, `LimitsOpts`,
`BrokerOpts`, `TlsOpts`); `Timing::fast()` scales everything down for tests.
Broker and parties should run with the same timing values.

The admin listener is `AdminOpts` (`--admin-bind`, `--admin-token` or
`NSM_ADMIN_TOKEN`), optional in `ListenOpts`; nothing else about it is
tunable.

Two limits only matter at a broker: `max_registrations` and
`max_store_bytes` (`--max-store-bytes`, default 16384, allowed 256 to
32768). `serve` accepts both with the other limits and ignores them.
`listen` refuses to start, with a configuration error naming both flags,
when a full store's reply (the budget plus 4096 bytes, which cover the
reply's own fields and the broker's reserved entries a `list` carries)
would not fit its own `--max-frame-bytes`, so a broker needs a frame limit
of at least 4352 bytes (20480 with the default budget). Parties use the
default 64 KiB frame, which every allowed budget fits. `nsm serve` uses its `--max-frame-bytes` for the parties
it starts and for every reply it reads itself, so lowering it below a store's
reply size breaks large replies there: at those parties, and as a 400 from
`POST /v1/store`. There is at most one store per client, and every
client holds a distinct service, so at most `max_registrations / 2` stores
exist: about 80 MiB of accounted store bytes with the defaults, and at most
64 stores for the parties of one host under the default per-host cap.

## 6. Security model

- **Rendezvous keys** select a service; they are not secrets in the sense of
  authentication (anyone who knows the key may claim). **Registration
  tokens** are: the broker issues one per registration, and pings, relays
  and the broker's own heartbeats must carry it. Refusals for an unknown id
  and a wrong token share one text so ids cannot be enumerated.
- **Admission** bounds what one host can do to the broker: the total
  registration cap, the per-host cap, and the optional requirement that the
  advertised address is the one the party connected from.
- **Transport security** is TLS with operator-configured trust anchors, no
  plaintext fallback, and TLS configuration checked before dialling. Mutual
  TLS and authorisation of `publish`/`claim` by identity are the listed
  follow-ups.
- **The shared store follows the claim.** Only a relay carrying a
  registered party's own token reaches a store, and only the store of that
  party's claim: the client's, or the one of the client holding the service
  at that instant. A removed party fails the token check, an unclaimed
  service can write nothing, and the next claimer of a service never sees
  the previous claim's data. The operator presents no token: **a party's
  listener is the capability**. Anyone who can reach a party's bind address
  can read and write its store through it, as with `send` and `collect`,
  and can learn where its peer listens for heartbeats (`nsm_mesh_data`),
  and TLS on that listener encrypts but does not authenticate callers
  (mutual TLS is a listed follow-up). The store is not a place for secrets. Store
  keys and values are never logged, and a `Store`'s `Debug` shows counts
  only.
- **The rendezvous key is a capability too.** `store_by_key` lets whoever
  knows a key read and write that key's claim's store at the broker, with
  no party address and no token. The key already lets its holder publish a
  service under it and be paired with the key's clients, so this adds no
  power the key did not give; what it changes is where the store can be
  reached from: the broker's address is fixed and reachable by every party,
  while a party's own listener behind NAT or in ping mode may not be. A
  refused resolution names party ids, which are not secrets.
- **The control plane** binds loopback by default, requires a bearer token
  elsewhere, takes no file paths from requests and limits body sizes.
- **The admin listener** is off by default and follows the same rule when
  on: loopback, or a bearer token on every request. It is read-only, but
  its status document lists every party with its key and bind address, an
  operator's view of the whole mesh; through the protocol listener a party
  learns only the addresses of its own claim (`nsm_mesh_data`), and
  whoever knows a rendezvous key those of that key's claim
  (`store_by_key`).
  It speaks plain HTTP; on a shared network it belongs on loopback behind an
  SSH tunnel, or behind a TLS-terminating proxy.
- **Resource bounds** everywhere: frame sizes, connection counts, request
  timeouts, registration counts; malformed input never reaches a handler.

## 7. Failure handling

| Event | Effect |
|---|---|
| a two-sided party stops answering | removed after `fail_threshold` failed heartbeats (each bounded by `heartbeat_timeout`); an acknowledgement resets the count |
| a ping-mode party falls silent | removed once its last contact is older than `ping_staleness` |
| a service is removed | each of its clients is re-paired with an unclaimed service of the same key and told in its next heartbeat; a client with no replacement is removed |
| a service is removed, the client re-paired | the claim's store stays where it is: the client keeps using it while orphaned, and the replacement reads every earlier write, the dead service's included, with versions continuing |
| a client is removed | its service becomes unclaimed; the claim's store is dropped, and the next claim of that service starts with an empty one |
| a store relay arrives from a removed party | refused with `unknown party or wrong token`, as its token no longer verifies |
| a put does not fit the store's budget | refused with `store full: ...`; the store is unchanged |
| a party stops hearing its broker | `Session::run` returns `Err(BrokerLost)`; the process exits 1 |
| a heartbeat's payload cannot be delivered | the pending inbox text or pairing is restored and carried by the next heartbeat |
| the broker shuts down | every connection and task is cancelled; parties notice through their watchdog |

Text delivery is "last message wins" per party: a second `send` before the
receiving party's next heartbeat replaces the first. The store is "last
writer wins" per key unless a write states an `if_version`, which the broker
compares in the same critical section that applies the write, and it lives
in broker memory only: a broker restart loses every store with every
registration.

## 8. Errors

`Error` is one `thiserror` enum. The binary maps every variant to exit code 1
with its `Display` text; the control plane maps input errors (`Json`, `Addr`,
`Config`, `Protocol`, `Rejected`, `WrongRole`, `NoService`,
`AmbiguousAddress`, `FrameTooLarge`) to 400, unreachable peers (`Timeout`, `BrokerLost`,
`PeerLost`, `Closed`, `Resolve`, connection-level `Io`) to 502, and the rest
to 500. `Error::is_disconnect` tells transient peer loss from local
misconfiguration; registration retries only on the former.

## 9. Tests

- **Unit tests** next to the code, including randomized ones driven by the
  seeded generator in `testing.rs` (address grammar, framing, store keys, the
  store's byte accounting, a churn of claims and store operations checked
  against a model) and scripted peers for the monitor's decisions.
- **`tests/e2e.rs`**: a broker with services and clients in one process on
  ephemeral loopback ports, over all four transports, with `Timing::fast()`
  and generated certificates, including the shared store across
  re-pairings, the end of a claim, ping mode and concurrent writers.
- **`tests/rest.rs`**: every control-plane route, the job lifecycle, status
  mapping and the token.
- **`tests/admin.rs`**: the admin listener over a running cluster: every
  gauge and counter against what the test did, including a service dying
  and a re-pairing; the status document; the token. The harness starts an
  admin listener on every cluster (`Cluster::admin_url`).
- **`tests/cli.rs`**: the built binary: parsing, exit codes, stdout/stderr
  discipline, a closed stdout, complete sessions with the store, SIGTERM.
- **`tests/stress.rs`** (`--ignored`): 50 services and 50 clients, text and
  store traffic, churn.
- CI enforces formatting, clippy for both providers, rustdoc, cargo-deny,
  cargo-machete, a Docker build and a line-coverage floor.

## 10. Build

Two mutually exclusive features select the rustls crypto provider:
`aws-lc-rs` (default; needs a C toolchain) and `ring` (pure Rust; used for
static musl builds and the Docker image). Dependencies are vendored;
`.cargo/config.toml` makes every build offline. Release builds keep overflow
checks on.

## 11. Design decisions

Numbered as in the 2026 refactor plan, because the code and the changelog
cite them by number (`decision D7` in the registry, `D9` in the control
plane, `D10` in the TLS module). Each is a fact about the current code. D13
to D16 come from the peer-address and two-way text plan of September 2026
([`history/2026-peer-text/`](history/2026-peer-text/PLAN.md)), where they
are decisions P1 to P10; D17 to D20 from the shared-store plan of the same
month ([`history/2026-shared-store/`](history/2026-shared-store/PLAN.md)),
where they are decisions S1 to S12; D21 to D24 from the monitoring plan of
October 2026 ([`history/2026-monitoring/`](history/2026-monitoring/PLAN.md)),
where they are decisions M1 to M10.

| # | Decision |
|---|---|
| D1 | Dependencies are vendored: `vendor/` is tracked, `.cargo/config.toml` replaces crates.io with it, and every build is offline. A change to `Cargo.lock` is followed by `cargo vendor` in its own commit, with nothing under `vendor/` left untracked. |
| D2 | One binary, `nsm`, with one subcommand per operation. The transport comes from the address scheme (`host:port`, `tls://`, `http://`, `https://`); `listen` and `serve` have no peer address and take `--transport` or `--bind`. |
| D3 | The wire format is versioned (`PROTOCOL_VERSION`): a tagged JSON message in length-prefixed frames on TCP and TLS or as the body of `POST /v1/message` on HTTP; every reply has one shape; registration replies carry the server-assigned id and a registration token. There is no compatibility with the pre-2026 `tcp` and `api` binaries. |
| D4 | HTTP servers are axum and HTTP clients are reqwest, both on hyper 1 with rustls. |
| D5 | Logging is `tracing`, configured by `NSM_LOG_LEVEL`, `NSM_LOG_STYLE` and `--log-level`; stdout carries only a command's result. |
| D6 | Interface enumeration uses `if-addrs`. |
| D7 | A claim is exclusive from the moment it is granted until the client is removed. There is no time-based lease. Services and clients are separate record types. |
| D8 | Liveness is one heartbeat task per two-sided party and a sweeper for ping-mode parties; a party is removed after `fail_threshold` consecutive failures. Every interval and threshold lives in `Timing`, overridable from the CLI; broker and parties should agree on the values. |
| D9 | The control plane binds loopback by default, requires a bearer token elsewhere, answers long-running operations with a job, and never takes file paths from a request. |
| D10 | Trust anchors are operator configuration (`--root-ca`, or `--system-roots` as an explicit opt-in) and never travel on the wire; a connection configured for TLS never falls back to plaintext. Mutual TLS is a follow-up ([#7](https://github.com/JBlaschke/nsm_rs/issues/7)). |
| D11 | The rustls crypto provider is a feature: `aws-lc-rs` (default) or `ring` (pure Rust, used for static musl builds and the container image); exactly one is installed per process. |
| D12 | Edition 2024 and `rust-version = "1.88"`, the minimum the current dependencies need; CI builds and tests on that toolchain as well as on stable. |
| D13 | A party's `Role` is a protocol type (`service` / `client`), and the reply to `collect` names it, so a reply says which of its fields apply instead of leaving the asker to guess; `ops::Collected` is an enum keyed by the role. Protocol version 2. |
| D14 | A client's pairing is a `tokio::sync::watch` channel (`Session::pairings`), not a slot: `nsm claim` prints one stdout line per pairing, the first at registration and one more each time the broker re-pairs it, so the last line is always the current service. |
| D15 | One verb per question: `nsm peer` prints a client's paired service and nothing else, `nsm collect` a party's last text and nothing else; asking a service for its peer is `Error::WrongRole`. A party that answered but has nothing to report yet is exit status 3, and a `store put` or `store delete` whose `--if-version` did not match (answered, nothing changed) is exit status 4 (1 is a failed operation, 2 a usage error). The control plane needs no `peer` route, since `POST /v1/collect` is typed. |
| D16 | Text flows both ways through one inbox per party: `send` at either party is relayed as a `deliver` that names no target, and the broker delivers to the sender's peer as it knows it (a client's current service, the client holding a service), so a text that races a re-pairing reaches the new service. Last text wins; a client's pending text survives a re-pairing. Protocol version 3. |
| D17 | A client and the service holding it share one key-value store, kept by the broker in the client's registry entry: created empty by `claim`, kept across re-pairings (so a replacement service reads what the dead one wrote), dropped when the client is removed. Parties keep no copy: each relays its request to the broker with its token (`store` becomes `store_relay`, as `send` becomes `deliver`), and the broker resolves whose store it is. The client always has access; a service only while it holds the claim, so an unclaimed service reads an empty store (`client: null`) and its writes are refused. |
| D18 | Four operations: get, put, delete (idempotent) and list (one atomic snapshot of every entry). Versions come from one broker-wide counter, like party ids, so a version names one write for the broker's whole life and never repeats across claims; the `stored` reply names the store (`client`, `revision`). |
| D19 | Store keys are one shell word (1 to 128 characters from `A-Z a-z 0-9 . _ - : /`, not starting with `-`), checked by `StoreKey` on parse and on decode; values are any text. One limit, `--max-store-bytes` per store (default 16384, 256 to 32768), counted in JSON-encoded bytes so that a full store's reply is bounded too; `listen` refuses a budget whose reply would not fit its frame limit. The store messages were new variants, so the protocol stays at version 3. |
| D20 | One command group, `nsm store get\|put\|delete\|list` (with `--json`), and one route, `POST /v1/store`. A put or delete may carry `if_version` (0: the key must be absent); a mismatch is an answer, not a failure: `applied: false` with the current entry, exit status 4, HTTP 409. Nothing is pushed or persisted: heartbeats carry no store data, and the store lives in broker memory for the claim's lifetime. |
| D21 | The broker counts what it does and reports it itself. A `Metrics` value owned by the `Broker`: counters are atomics bumped where the event happens (requests by kind and outcome, registrations granted and refused by reason, removals by role and reason, re-pairings, heartbeats by outcome with a round-trip histogram, store operations by operation and outcome), and gauges are read from the registry under its lock when asked (parties by role and mode, unclaimed services, failing parties, heartbeat tasks, stores with their entries and bytes, the limits), so they cannot drift. The Prometheus text exposition is written by hand, with no metrics crate; every metric is `nsm_*`, counters end in `_total`, units are in the name, and every label comes from a closed set, so no rendezvous key, host or party id ever becomes a time series. |
| D22 | One status document. `Status` (version, start time and uptime, the bound address, the limits and timing in force, the counts, a per-key and a per-host breakdown, the counters since start, every party) is the body of `GET /v1/status` and the input of `nsm status`, which prints `Status::summary`, or the document as one JSON line with `--json`; `--parties` adds a row per party, and `--watch SECS` repeats with a timestamp header and never clears the screen. The per-key and per-host breakdowns live here and not as metric labels. |
| D23 | A separate admin listener, off by default. `nsm listen --admin-bind ADDR` serves `GET /metrics`, `GET /v1/status` and `GET /healthz` over plain HTTP on a second socket parties never use, checked and bound before anything else starts. D9 applies: loopback, or `--admin-token` (`NSM_ADMIN_TOKEN`) on every request, compared in constant time. Nothing changes on the wire (`PROTOCOL_VERSION` stays 3); counters reset with the broker, and `nsm_start_time_seconds` says when. |
| D24 | A local stack, with and without containers. `deploy/monitoring/` runs Prometheus and Grafana on loopback by compose with the datasource and the `NSM broker` dashboard provisioned, scraping a broker on the host (token from a file) or one inside the stack (`--profile broker`); `scripts/monitoring-local.sh fetch\|start\|status\|stop` runs the same two servers as the current user from one work directory on hosts without containers, `fetch` checking the published SHA-256 sums of what it downloads. |

## 12. History

The code before September 2026 had two binaries (`tcp` and `api`) that
re-declared the same operations, a wire format that ended a message on a
short read, no tests and no library target. The refactor that replaced it is
recorded under [`history/2026-refactor/`](history/2026-refactor/PLAN.md): the
plan with its branch-by-branch status, and the audit of the old code with its
216 findings, which the code still cites by id (`audit S31`, `S20`, ...).

The September 2026 work on the client side, recorded under
[`history/2026-peer-text/`](history/2026-peer-text/PLAN.md), gave scripts a
reliable way to the paired service's address (`nsm peer`, one `claim` line
per pairing) and let text flow both ways; its decisions are D13 to D16.

The shared store of the same month, recorded under
[`history/2026-shared-store/`](history/2026-shared-store/PLAN.md), gave a
client and the service holding it a key-value store at the broker (`nsm
store`, `POST /v1/store`), with conditional writes; its decisions are D17 to
D20.

The monitoring work of October 2026, recorded under
[`history/2026-monitoring/`](history/2026-monitoring/PLAN.md), gave the
broker an admin listener with Prometheus metrics and a status document,
`nsm status` for a shell, and a Prometheus and Grafana stack for a laptop or
an interactive node; its decisions are D21 to D24.

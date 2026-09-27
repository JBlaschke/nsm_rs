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
  one copy of the store each claim shares. It never sees the service's own
  traffic.
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
| `broker` | `Registry` (pure, synchronous state), `Store` (the key-value store each claim shares with its service), `Broker` (monitor tasks and removal), `BrokerHandler` (admission and dispatch), `listen()` |
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
  not write.
- `store.rs` is `Store`: a key-value map with a byte budget, pure like the
  registry. Each entry counts as its JSON-encoded key and value plus 64
  bytes, which bounds the largest `stored` reply as well as memory; write
  numbers come from the registry's counter, passed in; `Debug` shows counts
  only.
- `monitor.rs` is `Broker`: the registry behind its mutex, one heartbeat task
  per two-sided party (`watch`), a sweeper task for ping-mode parties, and
  the single removal path `drop_party`, which also re-pairs or removes the
  clients a vanished service leaves behind. `snapshot()` exposes the state
  for status output and tests.
- `handler.rs` is `BrokerHandler`: admission (a real port, the optional
  matching-host check, the per-host cap) and one `match` over the request
  variants, written once for every transport. A relayed request (`deliver`,
  `store_relay`) checks the sender's token and acts on the registry in one
  critical section.
- `listen.rs` wires the three together with a transport listener, after
  checking that a full store's reply fits the frame limit.

### Party (`party`)

`Session::publish` and `Session::claim` bind the party's own listener first
(so the broker can dial it the moment registration succeeds), then register
with retries, then return a `Session` whose `run()` keeps the party alive:
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
`applied` false, a refusal is `Error::Rejected`), so the command line
and the control plane need no store logic of their own either: `nsm store`
gets the party and the `StoreOp` from `StoreCommand::into_parts` and only
prints (the value, the new version, the keys, or with `--json` the reply as
one line), and `POST /v1/store` returns the reply as its body, with status
409 and an `error` field when `applied` is false. `rest` serves
the same operations over HTTP: `publish` and `claim` become background jobs
with a view the API reports, cancels and reaps; a bearer token guards every
route when one is configured, and it is mandatory off loopback.

## 5. Configuration

All tunables live in `config`: `Timing` (heartbeat interval and timeout,
failure threshold, ping staleness, broker watchdog, request and connect
timeouts, registration retries, claim wait), `Limits` (frame size,
concurrent connections, registrations, store budget), `BrokerPolicy`
(matching-host check, per-host cap) and `TlsPaths`. Defaults are documented
on the types and overridable from the CLI (`TimingOpts`, `LimitsOpts`,
`BrokerOpts`, `TlsOpts`); `Timing::fast()` scales everything down for tests.
Broker and parties should run with the same timing values.

Two limits only matter at a broker: `max_registrations` and
`max_store_bytes` (`--max-store-bytes`, default 16384, allowed 256 to
32768). `serve` accepts both with the other limits and ignores them.
`listen` refuses to start, with a configuration error naming both flags,
when a full store's reply (the budget plus 1024 bytes) would not fit its own
`--max-frame-bytes`, so a broker needs a frame limit of at least 1280 bytes
(17408 with the default budget). Parties use the default 64 KiB frame, which every
allowed budget fits. `nsm serve` uses its `--max-frame-bytes` for the parties
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
  can read and write its store through it, as with `send` and `collect`, and
  TLS on that listener encrypts but does not authenticate callers (mutual
  TLS is a listed follow-up). The store is not a place for secrets. Store
  keys and values are never logged, and a `Store`'s `Debug` shows counts
  only.
- **The control plane** binds loopback by default, requires a bearer token
  elsewhere, takes no file paths from requests and limits body sizes.
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
writer wins" per key, and lives in broker memory only: a broker restart
loses every store with every registration.

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
are decisions P1 to P10.

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
| D15 | One verb per question: `nsm peer` prints a client's paired service and nothing else, `nsm collect` a party's last text and nothing else; asking a service for its peer is `Error::WrongRole`. A party that answered but has nothing to report yet is exit status 3 (1 is a failed operation, 2 a usage error). The control plane needs no `peer` route, since `POST /v1/collect` is typed. |
| D16 | Text flows both ways through one inbox per party: `send` at either party is relayed as a `deliver` that names no target, and the broker delivers to the sender's peer as it knows it (a client's current service, the client holding a service), so a text that races a re-pairing reaches the new service. Last text wins; a client's pending text survives a re-pairing. Protocol version 3. |

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

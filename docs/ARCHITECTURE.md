# Architecture

This document describes how the `nsm` crate is put together: the roles, the
modules, the rules every module follows, and why. For the wire format see
[`PROTOCOL.md`](PROTOCOL.md); for the control plane see
[`REST_API.md`](REST_API.md); for the reasoning behind the current design see
[`PLAN.md`](PLAN.md) and [the audit](audit/README.md) of the code it replaced.

## 1. Roles

```text
             ┌───────────────────────── broker (nsm listen) ─────────────────────────┐
             │  transport listener → BrokerHandler → Registry (pure state)           │
             │                        Broker monitor: heartbeat task per party,      │
             │                        sweeper for ping-mode parties                  │
             └───────────▲──────────────────────────────────────────────▲────────────┘
     publish / ping /    │                                              │  claim / ping /
     heartbeats          │                                              │  deliver / heartbeats
   ┌─────────────────────┴──────────┐                     ┌─────────────┴──────────────────┐
   │ service party (nsm publish)    │                     │ client party (nsm claim)       │
   │ Session + PartyHandler         │ ◄── data traffic ── │ Session + PartyHandler         │
   │ listener on the bind port      │   (not NSM's job)   │ listener on the bind port      │
   └─────────────────────▲──────────┘                     └─────────────▲──────────────────┘
                         │ collect                                      │ send / collect
                    operator, script, or `nsm serve` (REST control plane) driving `ops`
```

- The **broker** is the only fixed address. It admits registrations, pairs
  clients with services, monitors liveness and relays short texts. It never
  sees the service's own traffic.
- A **party** is a service or a client. Both run the same small server on
  their bind address and differ only in role: a service stores the text it is
  sent, a client stores the service it is paired with and relays `send`.
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
| `broker` | `Registry` (pure, synchronous state), `Broker` (monitor tasks and removal), `BrokerHandler` (admission and dispatch), `listen()` |
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
(what a client is told about its service) and the broker's records.

### Broker (`broker`)

- `registry.rs` is the model: services, clients, who holds whom, what is
  pending for whom. It is synchronous, does no I/O, never reads the clock
  (callers pass `now`), and is exhaustively unit-tested. Ids come from one
  counter that never repeats.
- `monitor.rs` is `Broker`: the registry behind its mutex, one heartbeat task
  per two-sided party (`watch`), a sweeper task for ping-mode parties, and
  the single removal path `drop_party`, which also re-pairs or removes the
  clients a vanished service leaves behind. `snapshot()` exposes the state
  for status output and tests.
- `handler.rs` is `BrokerHandler`: admission (a real port, the optional
  matching-host check, the per-host cap) and one `match` over the request
  variants, written once for every transport.
- `listen.rs` wires the three together with a transport listener.

### Party (`party`)

`Session::publish` and `Session::claim` bind the party's own listener first
(so the broker can dial it the moment registration succeeds), then register
with retries, then return a `Session` whose `run()` keeps the party alive:
in two-sided mode a watchdog that fails with `Error::BrokerLost` when the
broker's heartbeats stop for `broker_watchdog`; in ping mode a loop that
pings every `heartbeat_interval`, applies what the broker returns, and gives
up after `fail_threshold` failures or when the broker no longer knows the
party. `PartyHandler` answers the broker's `Heartbeat` (only with the party's
own token), `Collect`, and `Send` (relayed to the broker as `Deliver`).
`PartyState` is the shared state: id, token, inbox, paired service, last
contact.

### Operations and front-ends (`ops`, `cli`, `main`, `rest`)

`ops` turns typed requests into typed results. `main` parses the CLI, installs
the crypto provider and the signal handler, runs one operation, prints its
result to stdout and maps `Err` to exit code 1 with an `nsm: ` message on
stderr (clap's own usage errors exit 2). `rest` serves the same operations
over HTTP: `publish` and `claim` become background jobs with a view the API
reports, cancels and reaps; a bearer token guards every route when one is
configured, and it is mandatory off loopback.

## 5. Configuration

All tunables live in `config`: `Timing` (heartbeat interval and timeout,
failure threshold, ping staleness, broker watchdog, request and connect
timeouts, registration retries, claim wait), `Limits` (frame size,
concurrent connections, registrations), `BrokerPolicy` (matching-host check,
per-host cap) and `TlsPaths`. Defaults are documented on the types and
overridable from the CLI (`TimingOpts`, `LimitsOpts`, `BrokerOpts`,
`TlsOpts`); `Timing::fast()` scales everything down for tests. Broker and
parties should run with the same timing values.

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
| a client is removed | its service becomes unclaimed |
| a party stops hearing its broker | `Session::run` returns `Err(BrokerLost)`; the process exits 1 |
| a heartbeat's payload cannot be delivered | the pending inbox text or pairing is restored and carried by the next heartbeat |
| the broker shuts down | every connection and task is cancelled; parties notice through their watchdog |

Text delivery is "last message wins": a second `send` before the service's
next heartbeat replaces the first.

## 8. Errors

`Error` is one `thiserror` enum. The binary maps every variant to exit code 1
with its `Display` text; the control plane maps input errors (`Json`, `Addr`,
`Config`, `Protocol`, `Rejected`, `NoService`, `AmbiguousAddress`,
`FrameTooLarge`) to 400, unreachable peers (`Timeout`, `BrokerLost`,
`PeerLost`, `Closed`, `Resolve`, connection-level `Io`) to 502, and the rest
to 500. `Error::is_disconnect` tells transient peer loss from local
misconfiguration; registration retries only on the former.

## 9. Tests

- **Unit tests** next to the code, including randomized ones driven by the
  seeded generator in `testing.rs` (address grammar, framing) and scripted
  peers for the monitor's decisions.
- **`tests/e2e.rs`**: a broker with services and clients in one process on
  ephemeral loopback ports, over all four transports, with `Timing::fast()`
  and generated certificates.
- **`tests/rest.rs`**: every control-plane route, the job lifecycle, status
  mapping and the token.
- **`tests/cli.rs`**: the built binary: parsing, exit codes, stdout/stderr
  discipline, complete sessions, SIGTERM.
- **`tests/stress.rs`** (`--ignored`): 50 services and 50 clients, churn.
- CI enforces formatting, clippy for both providers, rustdoc, cargo-deny,
  cargo-machete, a Docker build and a line-coverage floor.

## 10. Build

Two mutually exclusive features select the rustls crypto provider:
`aws-lc-rs` (default; needs a C toolchain) and `ring` (pure Rust; used for
static musl builds and the Docker image). Dependencies are vendored;
`.cargo/config.toml` makes every build offline. Release builds keep overflow
checks on.

## 11. History

The code before 2026-09 had two binaries (`tcp` and `api`) that re-declared
the same operations, a wire format that ended a message on a short read, no
tests and no library target. The audit in [`audit/`](audit/README.md) lists
216 findings; [`PLAN.md`](PLAN.md) records the decisions taken (D1 to D12)
and the branch-by-branch path from that code to this one.

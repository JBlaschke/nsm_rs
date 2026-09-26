# Audit: Code quality and idioms

Findings reference `main` at commit `edd23a33` (2026-09-24), the state before the cleanup branches. Line numbers will drift as the cleanup lands; the file names are stable enough to locate each item.

Severity counts: 0 critical, 9 high, 16 medium, 4 low, 0 info.

## Summary

**Clippy (api bin = superset, 154 unique; tcp bin adds 2; ~160 with test targets).** By file: cli.rs 43, service.rs 38, api_builder.rs 29, connection.rs 17, operations.rs 11, network.rs 6, mode_api 5, mode_tcp 3, tls.rs 1, api.rs 1. **Mechanical (`cargo clippy --fix` safe, ~120):** redundant_field_names 40 (cli.rs:186-343, network.rs:61-62, connection.rs:50,133), needless_return 17, unnecessary_map_or 14 (api_builder.rs), needless_borrow 10, manual_map 8 (api_builder.rs:67,205,210,337,342,469,474,596,601), clone_on_copy 4, ptr_arg 3 (&String/&Vec<String>), cmp_owned 3, unwrap_or_default 3, partialeq_to_none 2, single_match 2, manual_ok_err 2, explicit_auto_deref 2, to_string_in_format_args 2, plus singles (bool_comparison service.rs:143, comparison_to_empty 262, useless_conversion 857, unnecessary_unwrap cli.rs:167, borrow_deref_ref, redundant_closure, single_component_path_imports api_builder.rs:12). **Design (must be fixed by hand, ~40):** let_underscore_future 5 = real bugs (futures never awaited: operations.rs:370, service.rs:532, 851, 1184 -- sleeps and a NULL reply that never run); let_unit_value 15 = `let _ = match {..}` used as statements and `let _ = fn()` discarding Results (tcp.rs:58-89, api.rs:76-108, service.rs:1390,1414,1441,1467, mode_*/operations.rs:48,55); upper_case_acronyms 11 = SCREAMING enum variants that are also the serde wire format (renaming needs `#[serde(rename_all)]` to stay compatible); empty_line_after_doc_comments 5 = `///` module docs attached to `use` lines (service.rs:1, connection.rs:1, network.rs:1) that should be `//!`.

**Design-level quality problems (details in findings):** (1) no error strategy: `std::io::Error::new(InvalidInput, ..)` fabricated for protocol/logic errors everywhere, `Box<dyn Error>` in mode_api, `Result<&mut Payload, u64>` with magic 1/2, CLI ops return `HttpResult` (an HTTP Response) and `Heartbeat::monitor` uses `StatusCode` as an internal state enum; 60 `let _ =`, 125 `unwrap()`, 29 `panic!`, 8 `std::process::exit(0)` inside library code; all `main` match arms discard Results so failures exit 0. (2) Transport dispatch by `(Option<TcpStream>, Option<Request>)` tuples with 12 `panic!("Unexpected state")` arms instead of an enum/trait; `Heartbeat{stream:Option, client:Option}` same pattern. (3) Types: i32 ports, String hosts, `Vec<String>` service_addr with `only_or_error` panics, bool pairs `print_v4/print_v6` meaning "use IPv4", `tls: bool` duplicating `Transport::HTTPS`, JSON-in-a-String double encoding (`Message.body: String` holding serialized `Payload`/`MsgBody`), `(i32,u64,u64,u64,i64)` tuple shared state addressed as `.0`..`.4`, `i64 = -1` sentinels with `as u64` casts, `as i32`/`as u64` truncation of JSON i64, `Value == "true"` comparisons that reject JSON booleans. (4) Duplication: HTTPS client builder x8, TLS accept/serve loop x3, http-vs-https request builder x6, cert base64 loader x2, watchdog task x2, JSON param extraction x4 (models already derive Deserialize), module tree declared twice (tcp.rs/api.rs) with no lib.rs. (5) Logging: broker prints state to stdout (`println!`) while CLI results also go to stdout with `{:?}`; `info!` logging full `Request` (headers) per request and per heartbeat; `eprintln!` for TLS errors; `error!("{}", format!(..))`. (6) Docs: every models.rs example cites a removed `nsm -o/--operation/--host --port` CLI; `verbose` help is inverted; "TCP stream present" comment in HTTP branch; ~20 commented-out code blocks; 53 TODOs. (7) Idioms: clap builder + hand-rolled string match + `assert!` argument validation instead of derive subcommands; `lazy_static` globals (GLOBAL_LAST_HEARTBEAT used by the lower-layer router; GLOBAL_MSGBODY limits a process to one message) instead of `LazyLock`/passed state; 13 `async fn` with zero awaits doing blocking fs/pnet I/O; `Arc<Mutex<String>>`/`Arc<Mutex<Addr>>` wrapping immutable inputs and `Arc<Mutex<closure>>` around a `Clone` closure locked per request; `State: Clone` cloned per event so `claim()` mutations are lost (service.rs:392,440); hand-rolled `Addr::from_str` while `url` is a dep; `ParseAddrError` implements neither Display nor Error.

## Detail

## Clippy categorization (api bin, 154 unique; whole workspace ~160)

| Strategy | Lint | Count | Notes |
|---|---|---|---|
| mechanical (`cargo clippy --fix`) | redundant_field_names | 40 | cli.rs:186-343, network.rs:61-62, connection.rs:50,133 |
| mechanical | needless_return | 17 | cli.rs:138,184,197,216,244,277,305,332; operations.rs:82; service.rs:633,678,696,699,1052 |
| mechanical | unnecessary_map_or | 14 | api_builder.rs (`map_or(true, ..)` -> `is_none_or`) |
| mechanical | needless_borrow / borrow_deref_ref / explicit_auto_deref | 13 | `& String` params, `&mut *lock` |
| mechanical | manual_map | 8 | api_builder.rs:67,205,210,337,342,469,474,596,601 |
| mechanical | clone_on_copy | 4 | service.rs:451,454 (bool), operations.rs:238,530 (closures) |
| mechanical | ptr_arg | 3 | utils.rs:4,13 `&Vec<String>`; connection.rs:46 `&String`; mode_*/operations.rs `host: &String` |
| mechanical | cmp_owned / comparison_to_empty / bool_comparison / partialeq_to_none | 7 | service.rs:143,262,1078,1238,1386; network.rs:98 |
| mechanical | unwrap_or_default, manual_ok_err, single_match, to_string_in_format_args, useless_conversion, redundant_closure, single_component_path_imports, unnecessary_unwrap | 13 | one-liners |
| **design** | let_underscore_future | 5 | **bugs**: operations.rs:370; service.rs:532, 851, 1184 -- add `.await`, handle result |
| **design** | let_unit_value | 15 | `let _ = op().await` in both mains and `let _ = match` statements -- symptom of discarded Results |
| **design** | upper_case_acronyms | 11 | Transport/ComType/MessageHeader variants are the serde wire format: rename + `#[serde(rename_all="UPPERCASE")]` (or accept a wire-format break in the same release as the Message enum redesign) |
| **design** | empty_line_after_doc_comments | 5 | `///` module docs on `use` lines -> `//!` (service.rs:1, connection.rs:1, network.rs:1) |

Recommended lint policy after cleanup (lib.rs): `#![deny(clippy::let_underscore_future, unused_must_use)]`, `#![warn(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::todo, clippy::cast_possible_truncation, clippy::cast_sign_loss, missing_docs)]`.

## Prioritized checklist by refactor branch

### Branch 1: `chore/hygiene` (no behavior change, unblocks everything else)
1. `git rm --cached` vendor/ (or keep + document offline policy), docs/, nsm-dev-buildx-latest.tar, src/.DS_Store, .env, view-events-rolebinding.yaml; fix .gitignore typos (`.yam`, `.keyl`), add `.env`, `*.tar`, `.DS_Store`, `/docs`.
2. Remove `threadpool`, `rustls-platform-verifier`, `lazy_static` (-> `LazyLock`), empty `ring`/`aws-lc-rs` features (Cargo.toml:15,19,27-29).
3. `cargo clippy --fix --allow-dirty` for the ~120 mechanical lints; `cargo fmt` (whole tree is unformatted: `& String`, `match com{`, trailing spaces).
4. `///` -> `//!` module docs (service.rs:1, connection.rs:1, network.rs:1); delete all commented-out code blocks (list in findings); `.version(env!("CARGO_PKG_VERSION"))` (cli.rs:13).
5. Fix stale models.rs examples (:9,25,45,72,113,154,182), inverted `verbose` help (cli.rs:88), `--root_ca` -> `--root-ca`, log "0.0.0.0.1" typo (api.rs:114), `NSM_LOG_STYLE` default `auto`.
6. Add `src/lib.rs` declaring the module tree once; tcp.rs/api.rs become `use nsm::*` wrappers. Enables `tests/` and single dead-code analysis. (Mechanical move; do it here so architecture branch diffs are readable.)
7. Add CI: `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, `cargo audit`/`cargo deny`.
8. Fix the 4 `let_underscore_future` bugs (add `.await`) -- tiny, high value, safe to land early.

### Branch 2: `refactor/architecture` (shared backend for TCP and REST)
1. Error type: `enum NsmError` (thiserror) + crate `Result<T>`; delete all `io::Error::new(InvalidInput, ..)`, `Box<dyn Error>`, `Result<_, u64>`; `main` returns `Result`/`ExitCode`; replace every `let _ = <Result>` with `?` or explicit logging.
2. Transport abstraction: `enum Peer { Tcp(..), Http(..) }` or `trait Transport { send/recv }`; rewrite `request_handler`, `heartbeat_handler(_helper)`, `Heartbeat::monitor` once against it; delete the 12 `panic!("Unexpected state")` arms and the `(Option, Option)` signatures.
3. Typed protocol: `#[serde(tag="type")] enum Message { Hb(MsgBody), Ack(Option<Payload>), Pub(Payload), Claim(Payload), Col, Msg(String), Null }`; length-delimited (or newline) framing via `tokio_util::codec`; pass `Payload`/`Addr` values, never their JSON strings.
4. Domain types: `port: u16`, `host: IpAddr`/`SocketAddr`, `service_addr: IpAddr`, `enum IpVersion`, drop `tls: bool` in favour of `Addr.transport`, `Option<u64>` instead of `-1`, `struct LastEvent` instead of the 5-tuple, `Duration` instead of `u64` seconds, `u32` fail counts.
5. Finish mode split then collapse: move claim/collect/send_msg out of operations.rs; one `https_client()`, one `build_request(addr, path, msg)`, one `accept_loop(listener, tls_acceptor, service)`, one `watchdog(last_hb)`, one `load_root_store`; delete the 7/5/2/1 duplicates.
6. Remove globals: `GLOBAL_LAST_HEARTBEAT`/`GLOBAL_MSGBODY` -> per-peer `Arc<PeerState>` passed into handlers; connection.rs must not import from operations.rs.
7. Remove `Clone` from `State`; event loop mutates through the shared `Arc<Mutex<State>>` (fixes lost `service_claim` update at service.rs:392/440); replace `Arc<Mutex<String>>`/`Arc<Mutex<Addr>>`/`Arc<Mutex<closure>>` with values or `Arc<T>`.
8. Make blocking helpers sync (`network.rs`, `tls.rs`, `State::claim/print`); call at startup or via `spawn_blocking`.
9. clap derive: `enum Operation` subcommands with the models.rs structs as `#[derive(Args, Deserialize)]`; delete cli.rs string match and `assert!`s; REST handlers `serde_json::from_slice::<Publish>(..)` -- one input type for both front-ends.
10. REST semantics: exact-match routes, POST for body requests, typed JSON results, no fire-and-forget "Successful request" replies; `collect` returns its value instead of printing.
11. Split service.rs (1498 lines) into state.rs / event_loop.rs / protocol.rs; `pub(crate)` by default; `Config` struct for all magic numbers.
12. Naming pass (safe once serde renames are in place): `Transport::{Tcp,Http,Https}`, `MessageKind::*`, `ip_version`, `peers`, `PeerTracker`, `remove`, `read_json_body`, `route_request`, `accept_loop`, `interface_name`, `root_certs_b64`.

### Branch 3: `fix/hardening` (after architecture; overlaps security lens)
1. Eliminate `unwrap()`/`panic!`/`process::exit` from non-main code (125/29/8 today); shutdown via `CancellationToken`; `#![warn(clippy::unwrap_used, clippy::panic)]`.
2. stdout/stderr discipline: stdout carries only the operation result (`{}`, optional `--json`); remove `State::print`/`println!` from the broker; `eprintln!` -> `error!`.
3. Log-level pass: per-request/per-heartbeat logging to `debug!`/`trace!`, never `{:?}` of `Request`; lifecycle at `info!`; failures currently at `trace!` promoted to `warn!`/`error!`. Consider `tracing` + spans.
4. Input validation at the edges: `u16::try_from` for ports, `as_u64` for keys, JSON booleans accepted as booleans, `Addr` via `FromStr` returning a `Display`able error.
5. Env-var config (CERT_PATH/KEY_PATH/ROOT_PATH) -> `Config` with clear errors instead of `expect`.
6. Tests to lock the above: unit tests for `Addr::from_str`, `Message` round-trip, `FailCounter`, `State::{add,claim,rmv}`; integration tests (now possible with lib.rs) spinning broker + publisher + claimant over loopback for both transports; property test for framing.
7. Dependency bump after tests exist (tokio, hyper 1.x stack, rustls 0.23 -> current, clap 4 derive, pnet -> consider `if-addrs`/`netdev` or `nix::ifaddrs` to drop the heavy pnet dependency since only interface enumeration is used; `lazy_static` -> std; `base64` engine API is fine).

## Findings

Ids are `Q` plus the finding number, in the order the reviewer reported them (not by severity).

| # | Severity | Category | Finding | Location |
|---|---|---|---|---|
| Q1 | high | error-handling | std::io::Error used as universal error type for protocol/logic failures | `src/operations.rs:350` |
| Q2 | high | error-handling | CLI operations return HttpResult (an HTTP Response) and HTTP StatusCode is used as an internal state machine | `src/operations.rs:48` |
| Q3 | high | error-handling | Every main() match arm discards the operation Result; failures exit 0 | `src/tcp.rs:58` |
| Q4 | high | correctness-idiom | Futures created but never awaited (let_underscore_future) -- sleeps and a protocol reply silently never run | `src/service.rs:851` |
| Q5 | high | error-handling | panic!/process::exit used for control flow inside library code (29 panics, 8 exits) | `src/service.rs:1217` |
| Q6 | high | error-handling | unwrap() on network-supplied data panics the broker (125 unwraps) | `src/connection.rs:195` |
| Q7 | high | architecture | Transport dispatch by (Option<TcpStream>, Option<Request>) tuples instead of an enum or trait | `src/service.rs:735` |
| Q8 | medium | types | Weak domain types: i32 ports, String hosts, Vec<String> single address, bool pairs for IP version, bool tls duplicating Transport | `src/connection.rs:41` |
| Q9 | medium | types | JSON-in-a-String double encoding: Message.body is a String that itself holds serialized JSON | `src/connection.rs:185` |
| Q10 | medium | types | Shared mutable state as an anonymous 5-tuple addressed by .0-.4 plus i64 sentinels and as-casts | `src/service.rs:305` |
| Q11 | medium | types | JSON boolean fields compared to the string "true" -- real booleans are rejected | `src/api_builder.rs:221` |
| Q12 | high | module-organization | Module tree declared twice for two bins; no lib.rs; 176 pub items | `src/api.rs:8` |
| Q13 | high | architecture | mode_tcp/mode_api extraction is half-finished: claim/collect/send_msg still contain inlined ComType branches | `src/operations.rs:344` |
| Q14 | medium | duplication | HTTPS client construction duplicated 8 times; request builder duplicated on tls flag 6 times | `src/tls.rs:105` |
| Q15 | medium | duplication | Certificate loading duplicated and mixed with base64 wire encoding in operations | `src/operations.rs:230` |
| Q16 | medium | logging | println!/eprintln! mixed with log macros; Debug formatting of user-facing output | `src/service.rs:770` |
| Q17 | medium | logging | info!-level logging on hot paths including full Request objects and secrets-adjacent input | `src/api_builder.rs:20` |
| Q18 | medium | docs | Stale and misleading documentation and comments | `src/models.rs:9` |
| Q19 | low | docs | Dead code kept as comments and 53 TODOs | `src/operations.rs:535` |
| Q20 | medium | idioms | clap builder with flat arg set, positional OPERATION string match, and assert!-based validation instead of derive subcommands | `src/cli.rs:11` |
| Q21 | medium | idioms | lazy_static process-wide mutable globals used for per-connection data | `src/operations.rs:53` |
| Q22 | medium | idioms | async fns that never await, performing blocking fs/pnet I/O on the tokio runtime | `src/network.rs:26` |
| Q23 | medium | idioms | Arc<Mutex<..>> wrapping immutable data and Clone closures; State cloned per event so claim() mutations are lost | `src/operations.rs:340` |
| Q24 | low | idioms | Hand-rolled Addr parser and error type without Display/Error; url crate already a dependency | `src/connection.rs:96` |
| Q25 | low | naming | Naming: SCREAMING variants (also the wire format), print_v4 meaning use-IPv4, misleading function names | `src/connection.rs:147` |
| Q26 | low | idioms | Hard-coded magic numbers scattered across the protocol | `src/service.rs:984` |
| Q27 | medium | design | Length-less TCP framing heuristic (bytes_read < buf.len()) with fixed 1024-byte buffer | `src/connection.rs:225` |
| Q28 | medium | design | api_builder handlers return success before work happens; collect overwrites error body; REST semantics off | `src/api_builder.rs:218` |
| Q29 | medium | hygiene | Repository hygiene: vendored deps, stale rustdoc, Docker tarball, .env and .DS_Store tracked; .gitignore typos | `.gitignore:1` |

### Q1. std::io::Error used as universal error type for protocol/logic failures

**Severity:** high  
**Category:** error-handling  
**Location:** `src/operations.rs:350`

Non-IO failures are fabricated as `std::io::Error::new(ErrorKind::InvalidInput, ..)`: operations.rs:350 ("Connection unsuccessful. Try another key"), :387 ("Key not found"), :394, :402 (wraps another io::Error in InvalidInput, losing kind), :669, :690, :803; service.rs:528 ("Service failed to connect to bind port"), :678 ("Failed to remove item from state"), :855 (`io::Error::new(..).into()` -- useless_conversion), :1258; connection.rs:58-67 (port range / transport mismatch). connection.rs:289 converts a serde_json::Error into io::Error via `e.into()`. mode_api/operations.rs:39,164 switch to `Box<dyn Error>` instead. service.rs:685 `State::claim` returns `Result<&mut Payload, u64>` with magic codes 1 and 2. Callers cannot distinguish failure classes (`err.kind()` matching at service.rs:177-195 only works for real socket errors).

**Fix:** Introduce one crate error enum (thiserror): `NsmError { Io(#[from] io::Error), Json(#[from] serde_json::Error), Http(#[from] hyper::Error), Tls(..), AddrParse(ParseAddrError), Protocol{expected, got}, KeyNotFound(u64), NoService(u64), Timeout, .. }` with a `type Result<T> = std::result::Result<T, NsmError>`; delete every `io::Error::new(InvalidInput, ..)`; make `State::claim` return `Result<&mut Payload, NsmError>`.

### Q2. CLI operations return HttpResult (an HTTP Response) and HTTP StatusCode is used as an internal state machine

**Severity:** high  
**Category:** error-handling  
**Location:** `src/operations.rs:48`

`pub type HttpResult = Result<Response<Full<Bytes>>, Error>` (operations.rs:48) is the return type of `listen` (:164), `publish` (:211), `claim` (:285), which are CLI entry points that end with `Ok(Response::new(Full::default()))` (:205, :280, :643) -- a dummy HTTP response for a terminal command. `event_monitor` returns `Result<Response<Full<Bytes>>, IoError>` (service.rs:294) and never returns. `Heartbeat::monitor` (service.rs:137) encodes liveness as `StatusCode::OK / BAD_REQUEST / REQUEST_TIMEOUT / GONE / ACCEPTED` (:180,:192,:248,:252,:269,:275,:280,:283) which `event_monitor` decodes with `status == StatusCode::ACCEPTED` / `status != StatusCode::OK` (:404-415). `tcp_server`'s handler type is `FnMut(Option<Arc<Mutex<TcpStream>>>) -> Pin<Box<dyn Future<Output = Result<Response<Full<Bytes>>, Error>>>>` (connection.rs:333-335) so the raw-TCP layer depends on hyper types. `ping_heartbeat` builds a full HTTP ACK response (service.rs:1187-1194) that no caller reads.

**Fix:** CLI ops return `Result<(), NsmError>` (or `Result<Output>` where Output is the printed data). `Heartbeat::monitor` returns a small `enum HbOutcome { Alive, Failed, PingStale, PingAlive }`. tcp_server handler returns `Result<(), NsmError>`; only api_builder handlers return `Response`.

### Q3. Every main() match arm discards the operation Result; failures exit 0

**Severity:** high  
**Category:** error-handling  
**Location:** `src/tcp.rs:58`

tcp.rs:58,64,70,76,81,85,89 and api.rs:76,82,88,94,99,103,107 all use `let _ = op(inputs, ..).await;` then `Ok(())`, so a failed claim/collect returns exit status 0 with nothing on stderr. Same pattern inside the library: `let _ = state_loc.add(..)` (service.rs:766,785,822,829) drops the error from connecting back to a peer; `let _ = stream_write(..)` (connection.rs:238,271; service.rs:163,821,1394,1418,1445,1470) ignores short/failed writes; `let _ = tcp_server(..)` (operations.rs:430; mode_tcp/operations.rs:41,112); `let _ = event_loop.await` (mode_tcp/operations.rs:53); `let _ = mode_api::operations::listen(..)` (operations.rs:199); 60 `let _ =` total.

**Fix:** `main` returns `Result<(), NsmError>` (or `anyhow::Result`) and propagates with `?`; use `std::process::ExitCode` for user-facing failures. Grep-and-replace every `let _ = <Result>` with `?`, `.map_err(..)?`, or an explicit `if let Err(e) = .. { warn!(..) }` with a comment saying why it is ignorable.

### Q4. Futures created but never awaited (let_underscore_future) -- sleeps and a protocol reply silently never run

**Severity:** high  
**Category:** correctness-idiom  
**Location:** `src/service.rs:851`

service.rs:851-854 `let _ = stream_write(&mut loc_stream, &serialize_message(&Message{header: NULL,..}));` has no `.await`: the NULL "claim failed" reply is never sent to a TCP claimant, which then waits for `stream_read` to time out. operations.rs:370 `let _ = sleep(Duration::from_millis(1000));`, service.rs:532 `let _ = sleep(..)` in the TCP connect-retry loop (so the 5 retries fire back-to-back with no backoff) and service.rs:1184 `let _ = sleep(Duration::from_millis(10000));` are all no-ops.

**Fix:** Add `.await` and handle the write result; enable `#![deny(clippy::let_underscore_future, unused_must_use)]` at crate root so this class cannot recur.

### Q5. panic!/process::exit used for control flow inside library code (29 panics, 8 exits)

**Severity:** high  
**Category:** error-handling  
**Location:** `src/service.rs:1217`

`std::process::exit(0)` on every read error class in `heartbeat_handler` (service.rs:1217,1221,1225,1229), in ping loop (:1171,:1179), in watchdog tasks (operations.rs:576; mode_api/operations.rs:276) -- exit code 0 for a failure, and unreachable from tests. `panic!` for expected runtime conditions: operations.rs:509 "Key not found. Try a different key.", :516, :522, :686 "Payload not found in heartbeat.", :765, :772, :773; mode_api/operations.rs:224,230; service.rs:1363,1369; `_ => panic!("Unexpected state: no stream or request.")` x12 (service.rs:258,738,793,831,865,1234,1382,1409,1433,1460,1484 and operations dispatch); `only_or_error` panics despite its name (utils.rs:7); cli.rs uses `assert!(args.contains_id(..))` (:209,:230-232,:261-264,:295,:320-321) and `panic!()` (:170,:347) for user input validation, `expect("CERT_PATH not set")` (tls.rs:22,34), `expect("ROOT_PATH not set")` (operations.rs:697,822).

**Fix:** Return `Err(NsmError::..)` and let the CLI layer decide exit codes; for long-running tasks use a shutdown channel (`tokio::sync::watch`/`CancellationToken`) instead of `process::exit`; replace `assert!` in cli.rs with clap `requires`/subcommands; make `only_or_error` return `Result`.

### Q6. unwrap() on network-supplied data panics the broker (125 unwraps)

**Severity:** high  
**Category:** error-handling  
**Location:** `src/connection.rs:195`

`deserialize_message(payload: &String) -> Message { serde_json::from_str(payload).unwrap() }` (connection.rs:195) and `deserialize(..) -> Payload` (service.rs:720) are called on every inbound TCP/HTTP body (connection.rs:248,262,292; service.rs:746-747,1213). `std::str::from_utf8(..).unwrap()` on raw socket bytes (connection.rs:222); `request.collect().await.unwrap()` (connection.rs:283); `serde_json::from_str(&message.body).unwrap()` (service.rs:877,929,1438,1465); `m.body.parse().unwrap()` (service.rs:343; mode_api/operations.rs:211-212); `collect_request(..).await.unwrap()` (operations.rs:498,760,882; service.rs:236,737,1233; mode_api/operations.rs:205); `TcpListener::bind(..).await.unwrap()` (connection.rs:342), `incoming.accept().await.unwrap()` (operations.rs:603; mode_api/operations.rs:93,309). Any malformed peer message kills the process.

**Fix:** Make (de)serialization helpers return `Result`; deserialize directly into typed structs; treat every `unwrap()` on data not created in the same function as a bug. Add `#![warn(clippy::unwrap_used)]` for non-test code once the count is down.

### Q7. Transport dispatch by (Option<TcpStream>, Option<Request>) tuples instead of an enum or trait

**Severity:** high  
**Category:** architecture  
**Location:** `src/service.rs:735`

`request_handler(state, stream: Option<Arc<Mutex<TcpStream>>>, request: Option<Request<Incoming>>)` (service.rs:728-731), `heartbeat_handler_helper(stream, request, payload, addr, tls)` (:995-997), `heartbeat_handler` (:1199-1202), and `Heartbeat { stream: Option<..>, client: Option<..> }` (:120-122) all encode "which transport" as two Options and match `(Some, None) / (None, Some) / _ => panic!` at :155-258, :735-738, :764-794, :816-832, :848-866, :904-912, :966-974, :1015-1049, :1207-1234, :1252-1383, :1390-1410, :1414-1434, :1441-1461, :1467-1485. Each protocol step is therefore written twice inline, which is exactly the duplication the user wants removed, and the invalid states are representable. `ComType` (connection.rs:140) exists but is not used to carry the connection.

**Fix:** Define `enum Peer { Tcp(Arc<Mutex<TcpStream>>), Http(HttpsClient, Uri) }` (or a `trait Transport { async fn send(&self, &Message) -> Result<Message>; async fn recv(..) }` with `TcpTransport`/`HttpTransport` impls). Protocol handlers (`request_handler`, `heartbeat_handler`, `Heartbeat::monitor`) then contain the logic once and call `peer.send(msg)`. This is the core of the shared-backend refactor.

### Q8. Weak domain types: i32 ports, String hosts, Vec<String> single address, bool pairs for IP version, bool tls duplicating Transport

**Severity:** medium  
**Category:** types  
**Location:** `src/connection.rs:41`

`Addr.port: i32` (connection.rs:41) forces `try_into()` at :54-62; ports are i32 in models.rs:60,96,135,137, service.rs:38,43, cli.rs:64-82 and `-1` is used as a sentinel (operations.rs:323). `Addr.host: String` (connection.rs:39) and `Heartbeat.addr: String` (service.rs:118) are re-parsed via `format!("{}:{}")` (connection.rs:200,341; service.rs:515). `Payload.service_addr: Vec<String>` (service.rs:36) but every consumer calls `only_or_error` (service.rs:514; operations.rs:182,261,318). `print_v4/print_v6: bool` (models.rs:52-54,85-88,124-127,158-161,186-189) mean "use IPv4/IPv6"; both can be true or false; cli.rs:164-175 derives them from an i32. `tls: bool` in every model plus `Transport::HTTPS` in `host` (api_builder.rs:216,348,480,607 `use_tls = host_addr.transport == Transport::HTTPS`) -- two sources of truth. `State::new(_tls: Option<ClientConfig>)` (service.rs:501) takes an ignored parameter. `State.timeout: u64` (service.rs:489) is unit-less seconds. `FailCounter.fail_count: i32` (service.rs:76) can never be negative.

**Fix:** `port: u16`; `host: IpAddr` or store `SocketAddr`/`url::Url`; `service_addr: IpAddr` (or `SocketAddr`); `enum IpVersion { V4, V6 }` (`Option<IpVersion>` for "both"); derive TLS from `Addr.transport` and drop `tls: bool`; `timeout: Duration`; `fail_count: u32`; remove the unused `_tls` parameter.

### Q9. JSON-in-a-String double encoding: Message.body is a String that itself holds serialized JSON

**Severity:** medium  
**Category:** types  
**Location:** `src/connection.rs:185`

`Message { header: MessageHeader, body: String }` (connection.rs:181-186). PUB/CLAIM put `serialize(&Payload)` in body (operations.rs:248,321), HB puts `serde_json::to_string(&MsgBody)` (service.rs:151,168,1113), MSG puts `serde_json::to_string(&inputs.msg)` of a `String` (operations.rs:792) producing a quoted JSON string inside a JSON string, and `rmv` returns `service_id.to_string()` in body (service.rs:660) which is later `.parse().unwrap()`ed (:343). Every consumer re-parses with unwrap (service.rs:746-747,877,929,1438,1465). `collect_request` parses to `serde_json::Value`, re-serializes to String, then re-parses to `Message` (connection.rs:285-293); same at service.rs:1154-1159, 1343-1346. `Payload` is also passed around as a serialized `String` and re-deserialized twice (service.rs:1079-1080, 1239; mode_api/operations.rs:210-213 deserialize->mutate->serialize).

**Fix:** `#[derive(Serialize, Deserialize)] #[serde(tag = "type", content = "body")] enum Message { Hb(MsgBody), Ack(Option<Payload>), Pub(Payload), Claim(Payload), Col, Msg(String), Null }` and deserialize straight from bytes (`serde_json::from_slice`). Pass `Payload`/`Addr` values, not their JSON.

### Q10. Shared mutable state as an anonymous 5-tuple addressed by .0-.4 plus i64 sentinels and as-casts

**Severity:** medium  
**Category:** types  
**Location:** `src/service.rs:305`

`let data = Arc::new(Mutex::new((0, 0, 0, 0, 0)));` with comment "(fail_count, key, id, service_id, fail_id)" (service.rs:304-305), then `shared_data.0 == 10 || shared_data.4 != 0` (:335), `rmv(shared_data.1, shared_data.2, shared_data.3)` (:337), `shared_data.4 as u64` (:362), `*data = (hb.fail_counter.fail_count, hb.key, hb.id, hb.service_id, fail_id as i64)` (:426-432, `fail_id` is already i64). `let mut service_id: i64 = -1;` sentinel (:306) compared via `hb.service_id == (service_id as u64)` (:438) and `hb.service_id as i64` (:410,:419). api_builder.rs:173,184,195,316,327,458,585 do `as i32`/`as u64` on JSON i64 values, silently truncating ports > 65535 or sign-flipping negative keys.

**Fix:** `struct LastEvent { fail_count: u32, key: u64, id: u64, service_id: u64, failed_service: Option<u64> }`; `Option<u64>` instead of -1; parse ports with `u16::try_from(..).map_err(..)` and keys with `as_u64()`; forbid `as` casts via `#![warn(clippy::cast_possible_truncation, clippy::cast_sign_loss)]`.

### Q11. JSON boolean fields compared to the string "true" -- real booleans are rejected

**Severity:** medium  
**Category:** types  
**Location:** `src/api_builder.rs:221`

`data.get("print_v4").map_or(true, |v| v == "true")` (api_builder.rs:221-222,231,353-354,362,483-484,609-610) compares a `serde_json::Value` with `&str`; `Value::Bool(true) == "true"` is false, so a client sending `{"ping": true}` gets `ping = false` and must send `"ping": "true"`. Query params are handled the same way (:29-30, :64-65) with inconsistent names (`v4`/`v6` vs `get_v4`/`get_v6`). Strings are `trim_matches('"')`ed after `as_str()` (:140,151,206,211,...) which is redundant. The 4 handlers hand-extract ~8 fields each (110-213, 253-345, 384-477, 510-604) although `Publish`, `Claim`, `Collect`, `SendMSG` already `#[derive(Deserialize)]` (models.rs:10,26,49,83,122,156,184).

**Fix:** `let req: Publish = serde_json::from_slice(&body)?` with `#[serde(default)]` on optional fields and a custom `Deserialize`/`FromStr` for `Addr`; return 400 with the serde error. Delete the manual extraction.

### Q12. Module tree declared twice for two bins; no lib.rs; 176 pub items

**Severity:** high  
**Category:** module-organization  
**Location:** `src/api.rs:8`

tcp.rs:8-23 and api.rs:8-25 each declare `mod service; mod models; mod tls; mod network; mod mode_api; mod mode_tcp; mod connection; mod utils; mod cli; mod operations;` (api.rs adds `mod api_builder`). Consequences: the crate compiles twice, `dead_code` analysis differs per bin (tcp bin compiles all of mode_api/api_builder deps), nothing can be integration-tested from `tests/` because there is no library target, and 176 `pub` items (vs 11 private fns) exist only so the sibling modules can see each other. Layering is inverted: connection.rs:3 imports `crate::operations::GLOBAL_LAST_HEARTBEAT` (transport layer depends on CLI layer); operations.rs:16-17 imports `mode_api`/`mode_tcp` while mode_*/operations.rs:5-10 import `crate::operations::{AMState, HttpResult, GLOBAL_LAST_HEARTBEAT}` (cycle). service.rs is 1498 lines mixing state, event loop, protocol handlers and HTTP client construction.

**Fix:** Add `src/lib.rs` with the module tree and `pub use` a small API; bins become `use nsm::..;` thin wrappers (or a single bin with `tcp`/`api` subcommands). Use `pub(crate)` by default. Split service.rs into `state.rs` (State/Payload/Heartbeat), `event_loop.rs`, `protocol.rs` (request_handler/heartbeat_handler). Move `Sem`/`AMState`/`HttpResult` aliases to the layer that owns them so transport never imports from operations.

### Q13. mode_tcp/mode_api extraction is half-finished: claim/collect/send_msg still contain inlined ComType branches

**Severity:** high  
**Category:** architecture  
**Location:** `src/operations.rs:344`

Only `listen` and `publish` were moved (mode_tcp/operations.rs:19-115, mode_api/operations.rs:37-345). `claim` (operations.rs:344-640, ~300 lines), `collect` (:661-776) and `send_msg` (:796-892) still `match com { ComType::TCP => {..}, ComType::API => {..} }` inline. The API half of `claim` (:432-639) is a near-verbatim copy of mode_api `publish` (mode_api/operations.rs:174-345): same TLS setup, same retry loop, same "Not implemented" handler (operations.rs:530-549 vs mode_api/operations.rs:238-255), same watchdog (:557-580 vs :257-280), same accept/serve loop (:602-638 vs :308-344 and :92-129).

**Fix:** Finish the split: `mode_tcp::{listen, publish, claim, collect, send}` and `mode_api::{..}` with identical signatures behind a `trait Mode` (or the `Peer` enum), and keep only IP selection/payload construction in operations.rs. Then collapse mode_* into the shared backend by parameterizing on the transport.

### Q14. HTTPS client construction duplicated 8 times; request builder duplicated on tls flag 6 times

**Severity:** medium  
**Category:** duplication  
**Location:** `src/tls.rs:105`

`HttpsConnectorBuilder::new().with_tls_config(..)/.with_native_roots().https_or_http().enable_http1().build()` + `Client::builder(TokioExecutor::new()).build(..)` appears at tls.rs:105-141 (`setup_https_client`, takes base64 PEM string), mode_api/operations.rs:135-157 (`get_https_connector`, returns a Client despite the name), operations.rs:459-474, :709-724, :834-849, service.rs:1085-1103, :1277-1294 and inline in tls.rs:124-136. Because the connector is `https_or_http`, the only tls-dependent difference in requests is the URI scheme, yet requests are built twice per site: operations.rs:729-750, :853-874; service.rs:216-229, :1123-1146, :1312-1335 with `match tls { Some(_t) => "https://..", None => "http://.." }`. `Addr` already implements `Display` with the scheme (connection.rs:74-81) but callers format `"{}:{}", host, port` instead.

**Fix:** One `fn https_client(root: Option<&RootCertStore>) -> Result<HttpsClient>` in tls.rs; one `fn build_request(addr: &Addr, path: &str, method, msg: &Message) -> Request<Full<Bytes>>` that uses `addr.to_string()`; delete the other 7/5 copies.

### Q15. Certificate loading duplicated and mixed with base64 wire encoding in operations

**Severity:** medium  
**Category:** duplication  
**Location:** `src/operations.rs:230`

operations.rs:230-245 and :301-316 are identical blocks that open `root_ca`, parse PEM, base64-encode each DER and join with '\n' to ship in `Payload.root_ca: Option<String>` (service.rs:51 docs it as "path to root store" -- it is actually inline certs). `load_ca` (tls.rs:45-76) and `setup_https_client` (tls.rs:105-141) then decode again with `.unwrap()` per line (:111). `tls_config` (tls.rs:79-100), `get_tls_acceptor` (:144-165) and operations.rs:436-451 re-implement the same server-config + root-store steps. Crypto-provider install is repeated in tls.rs:81-84, operations.rs:651-654, :785-788 under `cfg(feature)` gates for features that are declared empty in Cargo.toml:27-29 and therefore do nothing.

**Fix:** `tls.rs` exposes `load_root_store(path) -> Result<RootCertStore>`, `server_config() -> Result<ServerConfig>`, `client_config(root) -> ClientConfig`; `Payload.root_ca` becomes `Option<Vec<CertificateDer>>` with serde base64 (or `#[serde(with)]`); drop the fake `ring`/`aws-lc-rs` features and install the provider once in `main`.

### Q16. println!/eprintln! mixed with log macros; Debug formatting of user-facing output

**Severity:** medium  
**Category:** logging  
**Location:** `src/service.rs:770`

The broker prints its state to stdout with `println!("Now state:")` + `state_loc.print()` on every PUB/CLAIM (service.rs:770-771,790-791,872-874,707) and `println!("Removed item: {:?}", item)` (:646), so a listener's stdout is unusable as a data stream. CLI results are printed with `println!("{:?}", message.body)` (operations.rs:688,767) which wraps the message in quotes and escapes it, and `println!("Received payload: {}", ..)` (:379,:502). `println!("{:?}", result)` debug-dumps every HTTP result in the publish retry loop (mode_api/operations.rs:200). Server code uses `println!` for errors (api.rs:124,144) and `eprintln!` for TLS handshake failures (operations.rs:624; mode_api/operations.rs:115,330). `error!("{}", format!(..))` (tls.rs:25,37,50-51) and `trace!("{}", format!(..))` (service.rs:531). Default `NSM_LOG_STYLE=always` (tcp.rs:47, api.rs:60) forces ANSI codes into piped logs.

**Fix:** Rule: stdout = machine-readable result of the operation only (one line, `{}` not `{:?}`, optional `--json`); everything else via `log`/`tracing` to stderr. Replace `State::print` with `Debug`/`Display` at `debug!`. Use `write_style_or("NSM_LOG_STYLE", "auto")`.

### Q17. info!-level logging on hot paths including full Request objects and secrets-adjacent input

**Severity:** medium  
**Category:** logging  
**Location:** `src/api_builder.rs:20`

`info!("Entering handle_* with request: {:?}", request)` logs every request including all headers (api_builder.rs:20,55,111,254,385,511). Per-connection `info!("Passing TCP connection to handler...")` (connection.rs:349), per-request `info!("Starting request handler")` (service.rs:732), per-heartbeat `info!("altering last heartbeat: {:?}", hb)` (service.rs:960, dumps the whole Heartbeat including the Client), per-ping `info!("Request acknowledged: {:?}", m)` (service.rs:1161,1348; operations.rs:884), `info!("Dropping event")` (service.rs:477). `info!("Starting 'listen' operation with input: {:?}")` (operations.rs:165,212) dumps inputs including `root_ca` path. Meanwhile actual failures are logged at `trace!` (service.rs:179,185,191,418; operations.rs:575) or `debug!` (mode_tcp/operations.rs:50-51 "event monitor error").

**Fix:** Adopt levels: error = operator action needed, warn = degraded, info = lifecycle (startup, bind, peer added/removed), debug = per-request, trace = payload dumps. Log `request.method()`/`uri()` not `{:?}` of the Request. Consider `tracing` with spans keyed by peer id.

### Q18. Stale and misleading documentation and comments

**Severity:** medium  
**Category:** docs  
**Location:** `src/models.rs:9`

models.rs examples cite a CLI that no longer exists: `./target/debug/nsm -o list_interfaces` (:9), `nsm -n en0 -o list_ips` (:25), `--operation listen` (:45), `--operation claim --host 127.0.0.1 --port 8000` (:72; host is now positional `<IP>:<port>`, `--port` is gone), same at :113, :154 (`collect` example lacks the required host/key), :182 (`send` example has trailing spaces and no args). Binaries are `tcp`/`api` (Cargo.toml [[bin]]) and docs/ (156MB) is rustdoc for `nsm`. cli.rs:88 `verbose` help says "Don't output headers" but verbose *adds* headers (operations.rs:97,108). cli.rs:13 version "1.0" vs package 0.1.0. service.rs:213 "TCP stream present => Sending HB over TCP." is in the HTTP-client branch. service.rs:396 "use a worker from threadpool" (no threadpool). service.rs:677 "Convert the IoError to a HyperError" above an io::Error construction. connection.rs:297-298 api_server doc is a copy of tcp_server's ("Binds to stream and listens"). models.rs:162 Collect.host "claim's local ip address". service.rs:723 "use temples". `///` used as module docs on `use` lines (service.rs:1, connection.rs:1, network.rs:1). tcp.rs:33-38/api.rs:44-49 list 5 of 7 operations. README.md is 97 bytes.

**Fix:** Rewrite models.rs examples to the real invocation (or delete them and let clap `--help` be the source of truth via derive doc-comments); `//!` for module docs; `.version(env!("CARGO_PKG_VERSION"))`; delete docs/ from git and generate on CI; write README with protocol description, ports, env vars (CERT_PATH/KEY_PATH/ROOT_PATH/NSM_LOG_LEVEL).

### Q19. Dead code kept as comments and 53 TODOs

**Severity:** low  
**Category:** docs  
**Location:** `src/operations.rs:535`

Commented-out code blocks: operations.rs:535-541,663,798,821,827; service.rs:1050-1051,1063,1351-1357; mode_api/operations.rs:243-247,282; api_builder.rs:445-455,634-640; tls.rs:61-64; network.rs:95; api.rs:159-161; connection.rs:359. 53 `TODO`s, many describing the refactor itself ("this should be organized better" tcp.rs:7/api.rs:7; "handle errors" x6; "tidy up" x8; "Don't exist proc insitu" x7; "can we do this without mutable borrow?" x4; "Check if task_response is returned" x4). `#[allow(unused)]` hides dead `only_or_none` (utils.rs:12-18) and `epoch` (:21; actually used). `#[allow(dead_code)]` on `State::claim` (service.rs:684) which is used. `use std::marker::Send` (connection.rs:9; operations.rs:24; mode_api/operations.rs:16) is a prelude item.

**Fix:** Delete commented code (git has it). Convert each TODO into either a fix in the corresponding branch or a tracked issue; forbid new ones with `clippy::todo`. Remove the `#[allow]`s and let the compiler report.

### Q20. clap builder with flat arg set, positional OPERATION string match, and assert!-based validation instead of derive subcommands

**Severity:** medium  
**Category:** idioms  
**Location:** `src/cli.rs:11`

cli.rs:11-139 builds one `Command` where every flag is `required(false)` and applies to all 7 operations; cli.rs:161-349 then `match operation.as_str()` and enforces per-operation requirements with `assert!(args.contains_id(..))` (panics with an assertion message instead of usage text) and `.unwrap()` on `get_one`. `--ip-version` is `i32` and any value other than 4/6 panics (:170). `--root_ca` uses underscore while every other flag uses hyphens (:121). `--tls` is parsed for all ops but only meaningful for the API mode (:176 TODO). `--help` cannot show which flags belong to which operation. models.rs structs duplicate the arg list a third time and derive `Serialize, Deserialize` only so the REST layer can reuse them.

**Fix:** Switch to `#[derive(Parser)]` with `#[command(subcommand)] enum Operation { ListInterfaces(ListInterfaces), ListIps(ListIps), Listen(Listen), .. }` where the models.rs structs become `#[derive(Args, Deserialize)]`; ports as `u16`, `ip_version: Option<IpVersion>` via `ValueEnum`, `host: Addr` via `value_parser` using `FromStr`. This deletes cli.rs almost entirely and makes CLI and REST share one input type.

### Q21. lazy_static process-wide mutable globals used for per-connection data

**Severity:** medium  
**Category:** idioms  
**Location:** `src/operations.rs:53`

`GLOBAL_LAST_HEARTBEAT: Arc<Mutex<Option<Instant>>>` (operations.rs:53-55, alias `Sem` at :49 which is not a semaphore) is written by the router `api_server` (connection.rs:316-317) on any GET /heartbeat_handler and read by watchdog tasks (operations.rs:559-577; mode_api/operations.rs:259-277) -- the transport layer depends on a global owned by the CLI layer. `GLOBAL_MSGBODY: Mutex<MsgBody>` (service.rs:68-70) stores "the" pending message for the process (service.rs:1388,1464), so a service process can hold exactly one message regardless of how many clients. `lazy_static` is unnecessary since Rust 1.80 (`std::sync::LazyLock`), and toolchain is 1.98.

**Fix:** Remove both globals: pass an `Arc<PeerState { last_heartbeat: Mutex<Option<Instant>>, inbox: Mutex<VecDeque<MsgBody>> }>` into the handler closures. If a global is truly needed, use `static X: LazyLock<..>` and drop the `lazy_static` dependency.

### Q22. async fns that never await, performing blocking fs/pnet I/O on the tokio runtime

**Severity:** medium  
**Category:** idioms  
**Location:** `src/network.rs:26`

`get_local_ips` (network.rs:26), `ipstr_starts_with` (:68), `get_matching_ipstr` (:79) contain no `.await` (`pnet::datalink::interfaces()` is a blocking syscall); `get_interfaces` (operations.rs:58) awaits only these. tls.rs: `load_certs` (:21), `load_private_key` (:33), `load_ca` (:45), `tls_config` (:79), `setup_https_client` (:105), `get_tls_acceptor` (:144) do `fs::File::open` + PEM parsing synchronously inside `async fn`. `get_https_connector` (mode_api/operations.rs:135) is async with no await. `State::claim` (service.rs:685) and `State::print` (:704) are `async` with no await; `State::add`/`rmv` are async only because `deque` is a `tokio::sync::Mutex`. The `async` keyword hides that these block the executor and forces every caller into `.await` chains and `Pin<Box<dyn Future>>` handlers.

**Fix:** Make them plain `fn`; call once at startup (they are configuration), or wrap in `tokio::task::spawn_blocking` if they must run mid-flight. Use `std::sync::Mutex` for the deque if no `.await` happens while holding it.

### Q23. Arc<Mutex<..>> wrapping immutable data and Clone closures; State cloned per event so claim() mutations are lost

**Severity:** medium  
**Category:** idioms  
**Location:** `src/operations.rs:340`

`service_payload = Arc::new(Mutex::new("".to_string()))` (operations.rs:340) is written once (:381,:504) then only read; `broker_addr = Arc::new(Mutex::new(inputs.host.clone()))` (:410, TODO "clean up this pattern"), again at :595 and mode_api/operations.rs:298-299. `heartbeat_handler_helper(.., payload: Option<&Arc<Mutex<String>>>, addr: Option<&Arc<Mutex<Addr>>>, ..)` (service.rs:995-997) immediately `lock().await.clone()`s both (:1003,:1008); `ping_heartbeat` same (:1057-1071). Handlers are wrapped in `Arc<Mutex<closure>>` and locked+cloned on every request (mode_api/operations.rs:66,86; operations.rs:530,586) although the closure is `Clone` and the bound requires `Clone` (connection.rs:303). `State` derives `Clone` (service.rs:485) and `event_monitor` does `let mut state_clone = state_loc.clone()` (:392) then `state_clone.claim(hb.key)` (:440) -- the `service_claim` update is applied to a discarded copy.

**Fix:** Pass `Payload`/`Addr` by value or `Arc<Payload>`; handlers as plain `Clone` closures or `Arc<dyn Fn>`; remove `Clone` from `State` and operate through the shared `Arc<Mutex<State>>`.

### Q24. Hand-rolled Addr parser and error type without Display/Error; url crate already a dependency

**Severity:** low  
**Category:** idioms  
**Location:** `src/connection.rs:96`

`Addr::from_str` (connection.rs:96-134) strips a scheme by iterating `chars.next()` nchars times (:108-109), splits on ':' and re-joins (:111-128) to handle IPv6, and rejects '/' in host. `url::Url` is already used elsewhere (operations.rs:435, mode_api/operations.rs:181) to re-parse `Addr.to_string()`. `ParseAddrError` (:89-90) derives only Debug/PartialEq/Eq -- no `Display`/`std::error::Error`, so it cannot be used with `?` into a crate error and is printed with `{:?}` (api_builder.rs:166). `stream_write` (:204-209) is a match that re-wraps the identical Result. `Transport` derives `PartialEq` but not `Eq`/`Copy`; `ComType` derives Clone but not Copy/PartialEq.

**Fix:** Parse with `url::Url` (or `SocketAddr::from_str` for the raw case) and convert; `#[derive(Debug, thiserror::Error)] #[error("invalid address '{0}': expected [http(s)://]host:port")] pub struct ParseAddrError(String)`; `stream_write` becomes `stream.write_all(..).await`. Derive `Copy, Eq` on small enums.

### Q25. Naming: SCREAMING variants (also the wire format), print_v4 meaning use-IPv4, misleading function names

**Severity:** low  
**Category:** naming  
**Location:** `src/connection.rs:147`

`Transport::{SOCKET, HTTP, HTTPS}` (connection.rs:27-29), `ComType::{TCP, API}` (:141-142), `MessageHeader::{HB, ACK, PUB, CLAIM, COL, MSG, NULL}` (:149-161) -- these serde-derived names are the JSON wire format, so a rename must add `#[serde(rename_all = "UPPERCASE")]`. `print_v4/print_v6` (models.rs, cli.rs:164-175) mean "select IPv4/IPv6 address" in Listen/Claim/Publish/Collect/SendMSG. `SendMSG`, `ListIPs`, `CLIOperation` (cli.rs:143-158). `Sem` = `Arc<Mutex<Option<Instant>>>` (operations.rs:49). `State.clients` holds services too (service.rs:488). `Heartbeat` is a peer/connection record (:109). `rmv` (:637). `get_https_connector` returns a `Client` (mode_api/operations.rs:135). `collect_request` takes a body of a *response* in most call sites (connection.rs:282). `api_server` is a router, `tcp_server` is an accept loop (connection.rs:299,331). `heartbeat_handler_helper` (service.rs:995). `only_or_error` panics (utils.rs:4). `Payload.root_ca` documented as a path but holds base64 certs (service.rs:50-51). `name` means interface name (models.rs:35). api.rs:114 logs "0.0.0.0.1:8080".

**Fix:** `Transport::{Tcp, Http, Https}` with `#[serde(rename_all = "UPPERCASE")]`; `MessageHeader` -> `MessageKind::{Heartbeat, Ack, Publish, Claim, Collect, Message, Null}`; `ip_version: Option<IpVersion>`; `State.clients` -> `peers`/`registry`; `Heartbeat` -> `PeerTracker`; `rmv` -> `remove`; `collect_request` -> `read_json_body`; `api_server` -> `route_request`; `tcp_server` -> `accept_loop`; `interface_name`; `root_ca` -> `root_certs_b64`.

### Q26. Hard-coded magic numbers scattered across the protocol

**Severity:** low  
**Category:** idioms  
**Location:** `src/service.rs:984`

Fail threshold 10 (service.rs:335,433,465; comment at :126), retry counts 5 (operations.rs:386,508,515,521; service.rs:527,838; mode_api/operations.rs:223,229), 10 (service.rs:560,594,1169,1177), 50 (:885,924,937,986); sleeps 200ms (:316,480), 300ms (:839,922), 702ms (:984), 500ms (operations.rs:565), 1000ms, 2000ms (:1062), 5000ms, 10000ms; timeouts 3s (:232), 6s (connection.rs:215; operations.rs:341,726,850), 10s (:574,1082,1241), 60s (:279,574,504); buffer 1024 (connection.rs:213); `worker_threads = 20` (tcp.rs:41) but default runtime in api.rs:53; `"0.0.0.0:8080"` (api.rs:115). `FailCounter.interval` 5s (:93) interacts with the 200ms event loop so "10 failures" actually means >=50s.

**Fix:** One `struct Config { hb_interval, hb_timeout, max_failures, connect_retries, read_timeout, frame_buf, .. }` with `Default`, overridable by CLI/env; document the resulting liveness timings in the README.

### Q27. Length-less TCP framing heuristic (bytes_read < buf.len()) with fixed 1024-byte buffer

**Severity:** medium  
**Category:** design  
**Location:** `src/connection.rs:225`

`stream_read` loops `stream.read(&mut buf)` and stops when `bytes_read < buf.len()` (connection.rs:216-228). A message whose length is an exact multiple of 1024 bytes causes a second read that blocks until the 6s timeout and returns an error; a message split by the kernel into a short read is truncated and `deserialize_message(..).unwrap()` then panics; `from_utf8(&buf[..n]).unwrap()` (:222) panics if a multi-byte char straddles a read boundary. `receive` also auto-ACKs everything except ACK/HB (:266-276), which `send` does not read back for ACK messages (:240-245), leaving stray ACKs in the stream for the next reader.

**Fix:** Use `tokio_util::codec::{Framed, LengthDelimitedCodec}` (or newline-delimited JSON via `LinesCodec`) with a typed `Message` codec; define ack semantics once in the transport trait.

### Q28. api_builder handlers return success before work happens; collect overwrites error body; REST semantics off

**Severity:** medium  
**Category:** design  
**Location:** `src/api_builder.rs:218`

`handle_publish` and `handle_claim` `tokio::spawn` the operation and immediately reply "Successful request to publish/claim" (api_builder.rs:218-249, 350-380); the spawned task's `task_response` is built then dropped via `let _ = Ok::<..>(task_response)` (:245, :376). `handle_collect` sets a 500 + error body on `Err` then unconditionally overwrites the body with `"Successful request to collect: {:?}"` of the `()` result (:503-505), and `collect` itself prints the answer to the *server's* stdout (operations.rs:688,767) instead of returning it. `GET /claim` and `GET /collect` read a JSON request body (api.rs:150,153); routing uses `starts_with` (api.rs:140-158; connection.rs:311,314) so `/publishXYZ` matches. Collect/Send require `name` and `starting_octets` (TODOs at :411,:468,:539,:595).

**Fix:** Operations return typed results (`ClaimResult { service: Payload }`, `CollectResult { msg }`); handlers serialize them; long-running ops (publish/claim keep a heartbeat server alive) need a job/registry model with a status endpoint rather than fire-and-forget. Exact-match routes, POST for body-carrying requests.

### Q29. Repository hygiene: vendored deps, stale rustdoc, Docker tarball, .env and .DS_Store tracked; .gitignore typos

**Severity:** medium  
**Category:** hygiene  
**Location:** `.gitignore:1`

`git ls-files` shows 23,420 files under vendor/ and docs/; `nsm-dev-buildx-latest.tar` (18MB), `src/.DS_Store`, and `.env` containing another developer's CERT_PATH/KEY_PATH are tracked. `.gitignore` lists `view-events-rolebinding.yam` and `server.keyl` (typos) so `view-events-rolebinding.yaml` is tracked and a `server.key` would not be ignored. Cargo.toml declares `threadpool` (unused), `rustls-platform-verifier` (optional, never enabled), and empty features `ring`/`aws-lc-rs` (Cargo.toml:27-29) that are checked with `cfg(feature)` but enable nothing.

**Fix:** `git rm -r --cached vendor docs nsm-dev-buildx-latest.tar src/.DS_Store .env view-events-rolebinding.yaml`; fix .gitignore (`.env`, `*.tar`, `.DS_Store`, `/docs`, `/vendor` or keep vendor only if offline builds are a requirement -- then document it); remove `threadpool`, `rustls-platform-verifier`, fake features; add `cargo deny`/`cargo audit` in CI.


# Audit: Correctness and concurrency

Findings reference `main` at commit `edd23a33` (2026-09-24), the state before the cleanup branches. Line numbers will drift as the cleanup lands; the file names are stable enough to locate each item.

Severity counts: 2 critical, 11 high, 11 medium, 9 low, 2 info.

## Summary

## Correctness & concurrency audit of nsm_rs (HEAD edd23a33)

All 15 hypotheses (a)–(o) were checked against source; 12 confirmed, 2 partially confirmed/refined, 1 refuted. ~25 additional bugs found. Verified with `cargo clippy --offline` (160 warnings incl. 5 `let_underscore_future`) and `cargo tree --offline`.

**Systemic problems (each backed by a finding below):**
1. **Broker liveness is trivially killable from the network.** `tcp_server` (connection.rs:351) awaits the handler serially inside the accept loop, and `request_handler` `unwrap()`s/`panic!`s on any malformed, empty, or unexpected message (service.rs:736-750, connection.rs:222/195). One garbage or half-open TCP connection panics the spawned accept task; the broker keeps running the event loop but never accepts again.
2. **The 10-failure removal threshold is dead logic.** Any single non-OK `monitor()` result sets `fail_id = hb.service_id` (never 0 → service.rs:408-411), which both drops the heartbeat from the deque and removes the entity via `shared_data.4 != 0` (service.rs:335). `FailCounter`, `interval`, `first_increment` are irrelevant.
3. **Shared `data` tuple loses results** (service.rs:305/332/425): multiple in-flight monitor tasks overwrite each other; lost failures leave "ghost" payloads in `State.clients` that `claim()` hands out.
4. **Re-claim logic is a no-op on a cloned State** (service.rs:392/440) and never updates `hb.service_id` (447), so the `service_id` latch fires forever.
5. **`State::add` reconnect loop can spin forever while holding the State lock** (service.rs:560-593), freezing the event monitor → all peers time out and `process::exit`.
6. **send/collect messaging is broken end-to-end:** MSG lookup matches `e.id == msg_body.id` but relayed `MsgBody.id` is always 0 (service.rs:890 vs 1265/1298); peers never reply to the sender (1253-1273); ping-mode never delivers stored msgs (143/277).
7. **Two-sided API heartbeats panic the broker's monitor task** because peer handlers are `Not implemented` stubs returning 400 text (operations.rs:530-549, mode_api/operations.rs:238-255) and `monitor()` `unwrap()`s the JSON parse (service.rs:236).
8. **REST front-end is unreachable**: `operation` is `required(true)` (cli.rs:21) so `get_matches()` exits before `contains_id` (api.rs:63) can be false — `api_builder.rs` is 640 lines of dead code. Even if reached, publish/claim handlers return 200 before the op runs and bool params only work as the string "true".
9. **Framing:** `stream_read` (connection.rs:212-231) uses "short read = end of message": coalesced ACK+ACK → parse panic; exact-1024-multiple → 6s timeout; EOF → `""` → `deserialize_message` panic; `write()` not `write_all()` (204). 
10. **Ops/tooling drift:** Dockerfile CMD, `test_event_monitor.sh`, and models.rs doc examples all use the removed `-o/--operation`/`--port`/`nsm` CLI; `.gitignore` typos leave `server.key`/`*.yaml` unignored; `.env` with another dev's paths, 18MB tar, `.DS_Store`, 356MB vendor, 156MB stale docs tracked.

**Dead deps:** `threadpool` (Cargo.toml:15, never imported), `rustls-platform-verifier` (Cargo.toml:22, optional, never enabled, yet vendored), duplicate vendored `rustls-native-certs-0.7.3`; `lazy_static` replaceable by `std::sync::LazyLock`; `[features] ring/aws-lc-rs` are empty and don't forward to rustls, so all `install_default()` calls are `cfg`'d out and the provider works only by accident via hyper-rustls default feature `aws-lc-rs`.

## Detail

## Hypothesis scorecard
| # | Hypothesis | Verdict | Key lines |
|---|---|---|---|
| a | re-claim on cloned State | CONFIRMED (+ hb.service_id never updated, latch never reset) | service.rs:392,440,447,306 |
| b | five `let _ = sleep` | PARTIAL: 3 sleeps (service.rs:532,1184; operations.rs:370) + 1 lost `stream_write` NULL reply (service.rs:851) + 1 benign spawn (1017) | clippy `let_underscore_future` ×5 |
| c | `data` tuple race | CONFIRMED; plus block 357-370 is a permanent no-op | service.rs:332,425,355,362 |
| d | REST branch unreachable | CONFIRMED (clap required positional → exit before contains_id) | cli.rs:21,136; api.rs:63 |
| e | Dockerfile/test script stale | CONFIRMED (+ `en0` in alpine, `.env` never loaded) | Dockerfile:77; src/test_event_monitor.sh:5-9 |
| f | stream_read framing | CONFIRMED (coalescing, k×1024, EOF, UTF-8, partial write) | connection.rs:204-231 |
| g | tls from root_ca | CONFIRMED | service.rs:548 |
| h | ping keeps entry alive? | CONFIRMED-NEGATIVE for clients: pings carry the service's id; no MSG delivery in ping mode | service.rs:1078,944,279,143 |
| i | add() reconnect loop | CONFIRMED infinite loop under State lock (empty deque, or match older than 60s) | service.rs:560-593,574 |
| j | 200 before outcome | CONFIRMED; plus bool params only as string "true" | api_builder.rs:218-249,231 |
| k | collect overwrite | CONFIRMED | api_builder.rs:503 |
| l | first_increment | CONFIRMED (never on TCP path) | service.rs:177-207,251 |
| m | install_default each call | REFUTED as stated: cfg'd out entirely; features don't forward; `--features ring` won't compile | Cargo.toml:32-34; cargo tree |
| n | worker_threads | CONFIRMED inconsistency, low impact | tcp.rs:41; api.rs:53 |
| o | Addr edge cases | CONFIRMED: bare host:port panics in API mode via Url::parse; IPv6 bracket handling inconsistent between connect() and to_socket_tuple() | connection.rs:96-134,54,200; operations.rs:435 |

## Extra confirmed defects not in the hypothesis list
- Single failed heartbeat removes entity (fail_id path) — service.rs:408
- MSG lookup uses id 0 → send never works — service.rs:890 vs 1265
- Peer never replies to send_msg — service.rs:1253-1273
- API two-sided HB → broker monitor task panics on stub 400 — service.rs:236
- 6s peer exit + 200ms/event cycle → capacity ceiling ~30 TCP peers — service.rs:1223,480
- claim() returns clients as services and re-issues after 60s — service.rs:689
- State lock across connect/sleep — service.rs:762,806-839
- tcp_server serial + unwrap/panic chain → remote DoS of accept loop — connection.rs:351; service.rs:736-750
- only_or_error on remote payload — service.rs:514

## Metrics (verified)
- `cargo clippy --offline --all-targets`: 160 warnings; top: 40 redundant_field_names, 17 needless_return, 15 let_unit_value, 14 needless-`match`, 11 upper_case_acronyms, 10 needless_borrow, 10 manual impls, 5 let_underscore_future.
- `grep -c`: 125 `unwrap()`, 29 `panic!`, 8 `process::exit`, 53 TODO.
- Vendored versions: clap 4.5.23, tokio 1.42.0, hyper 1.5.2, hyper-util 0.1.10, hyper-rustls 0.27.5, rustls 0.23.20, tokio-rustls 0.26.1, rustls-native-certs 0.8.1 (+0.7.3 dup), pnet 0.33.0, url 2.5.4, env_logger 0.11.6, base64 0.22.1, rustls-pemfile 2.2.0, lazy_static 1.5.0, threadpool 1.8.1. Cargo.lock v4. Dockerfile pins rust 1.83; host rustc 1.98.1.
- Git: 158 commits; `.git` 340MB; 23,449 tracked files (12,449 vendor, 10,971 docs).

## Suggested test targets (currently 0 tests)
1. `Addr::from_str` table test (host:port, http://h:p/, [::1]:p, ::1:p, no port, path).
2. `stream_read`/`stream_write` framing round-trip incl. 1024-multiple and coalesced frames (use `tokio::io::duplex`).
3. `State::add/claim/rmv` unit tests: claim filters services only; rmv resets service_claim; reconnect path terminates.
4. Event-monitor integration test with a fake peer that (a) replies, (b) times out once, (c) dies — assert removal only after N failures.
5. Message send→relay→HB→collect end-to-end over loopback.
6. clap parse tests for each operation's required flags.

## Findings

Ids are `C` plus the finding number, in the order the reviewer reported them (not by severity).

| # | Severity | Category | Finding | Location |
|---|---|---|---|---|
| C1 | high | logic | (a) CONFIRMED: re-claim mutates a throwaway clone of State; service_claim update lost | `src/service.rs:392` |
| C2 | low | logic | (a-bis) `service_id` latch is never reset → re-claim path runs on every heartbeat forever | `src/service.rs:438` |
| C3 | high | async | (b) PARTIAL: 3 of the 5 `let_underscore_future` are sleeps; a 4th silently drops the NULL reply to a failed CLAIM | `src/service.rs:532` |
| C4 | high | concurrency | (c) CONFIRMED: shared `data` tuple races; lost failure results leave ghost entries in State.clients | `src/service.rs:332` |
| C5 | high | logic | Any single failed heartbeat removes the entity; the 10-failure threshold is dead logic | `src/service.rs:408` |
| C6 | high | dead-code | (d) CONFIRMED: REST server branch in api.rs is unreachable; api_builder.rs is dead code | `src/api.rs:63` |
| C7 | medium | ops | (e) CONFIRMED: Dockerfile CMD, test script and doc examples use a removed CLI | `Dockerfile:77` |
| C8 | high | protocol | (f) CONFIRMED: stream_read framing is 'short read = end of message' → panics, hangs, and coalescing bugs | `src/connection.rs:225` |
| C9 | critical | robustness | Broker accepts connections serially and panics on any malformed/unexpected message | `src/connection.rs:351` |
| C10 | medium | logic | (g) CONFIRMED: Heartbeat.tls derived from root_ca presence, not from transport | `src/service.rs:548` |
| C11 | high | logic | (h) CONFIRMED with detail: ping-mode client refreshes the SERVICE's heartbeat, not its own → removed after 60s | `src/service.rs:1078` |
| C12 | critical | concurrency | (i) CONFIRMED: State::add reconnect loop can spin forever holding the State lock | `src/service.rs:560` |
| C13 | medium | api | (j) CONFIRMED: handle_publish/handle_claim return 200 before the operation runs; spawned op never completes | `src/api_builder.rs:218` |
| C14 | low | api | (k) CONFIRMED: handle_collect overwrites error body with 'Successful' while leaving status 500 | `src/api_builder.rs:503` |
| C15 | low | logic | (l) CONFIRMED: FailCounter.first_increment updated only in 3 of 7 failure branches; never on TCP path | `src/service.rs:177` |
| C16 | medium | build | (m) REFUTED as stated; actual bug: crypto-provider features are empty so install_default() never compiles, and `--features ring` cannot build | `Cargo.toml:32` |
| C17 | info | consistency | (n) CONFIRMED (low impact): runtime config inconsistent between binaries | `src/tcp.rs:41` |
| C18 | medium | parsing | (o) CONFIRMED: Addr parsing/formatting inconsistencies; bare host:port panics in API mode | `src/connection.rs:96` |
| C19 | high | logic | send/MSG delivery never matches a heartbeat: relayed MsgBody.id is always 0 | `src/service.rs:890` |
| C20 | medium | protocol | TCP peer never replies to send_msg; sender times out after 6s | `src/service.rs:1253` |
| C21 | high | robustness | Two-sided API heartbeats panic the broker's monitor task; entity vanishes from deque but not from State | `src/service.rs:236` |
| C22 | high | liveness | Peer heartbeat_handler calls process::exit on any 6s idle read; broker cycle time scales with peer count | `src/service.rs:1223` |
| C23 | high | logic | State::claim treats any payload under the key as a service and re-issues claimed services after 60s | `src/service.rs:689` |
| C24 | medium | concurrency | State lock held across network I/O and sleeps in request_handler | `src/service.rs:762` |
| C25 | low | performance | event_monitor deep-clones State.clients under lock on every event | `src/service.rs:392` |
| C26 | low | logic | Single-slot GLOBAL_MSGBODY loses messages; broker re-sends the same msg on every HB | `src/service.rs:69` |
| C27 | medium | robustness | Request/response body parsing panics on malformed input in API mode (per-connection) | `src/connection.rs:283` |
| C28 | medium | robustness | only_or_error panics whenever the interface has ≠1 matching address | `src/utils.rs:7` |
| C29 | low | cli | cli.rs uses assert!/unwrap for missing optional flags; `collect`/`send` require flags they don't use | `src/cli.rs:209` |
| C30 | low | logic | GLOBAL_LAST_HEARTBEAT watchdog is refreshed by ANY GET /heartbeat_handler, including collect requests | `src/connection.rs:314` |
| C31 | medium | robustness | mode_tcp listen swallows accept-loop panics/bind failures; broker appears healthy while deaf | `src/mode_tcp/operations.rs:40` |
| C32 | medium | logic | heartbeat_handler_helper spawns a heartbeat loop for EVERY inbound connection, including collect/send | `src/service.rs:1017` |
| C33 | low | dependencies | Unused / misconfigured dependencies | `Cargo.toml:15` |
| C34 | info | dead-code | Dead code inventory | `src/service.rs:357` |
| C35 | low | repo | Repository hygiene: tracked secrets/paths, binaries, stale docs, .gitignore typos | `.gitignore:6` |

### C1. (a) CONFIRMED: re-claim mutates a throwaway clone of State; service_claim update lost

**Severity:** high  
**Category:** logic  
**Location:** `src/service.rs:392`

`let mut state_clone = state_loc.clone();` (392) deep-copies `clients` (State derives Clone, 485). The spawned task then calls `state_clone.claim(hb.key)` (440) which sets `service_claim = epoch` on the COPY. The real `State.clients` never records the claim. Furthermore the rebuilt Heartbeat keeps `service_id: hb.service_id` (447) instead of the new service's id, and the claimed payload `p` is only logged (442) - the client is never told the new service address.

**Fix:** Hold `Arc<Mutex<State>>` in the task and call `claim` on the real state; set `hb.service_id = p.service_id`; deliver the new payload to the client via the next HB; add a unit test for re-claim.

### C2. (a-bis) `service_id` latch is never reset → re-claim path runs on every heartbeat forever

**Severity:** low  
**Category:** logic  
**Location:** `src/service.rs:438`

`service_id: i64` (306) is set when a PUB is removed (343) and never reset to -1. It is copied into every spawned task (438). Because the rebuilt hb keeps the old `service_id` (447), `hb.service_id == service_id` stays true on every subsequent pass, so `claim` (on the clone) runs each 200ms cycle and `FailCounter::new()` (452) resets the counter every pass.

**Fix:** Reset the latch after processing; update hb.service_id on successful re-claim.

### C3. (b) PARTIAL: 3 of the 5 `let_underscore_future` are sleeps; a 4th silently drops the NULL reply to a failed CLAIM

**Severity:** high  
**Category:** async  
**Location:** `src/service.rs:532`

Clippy reports 5 `non-binding let on a future`: service.rs:532 `let _ = sleep(1000)` (State::add TCP connect retry - 5 retries happen in microseconds while holding the State lock), service.rs:1184 `let _ = sleep(10000)` (ping outer loop - no delay), operations.rs:370 `let _ = sleep(1000)` (claim read loop), service.rs:851 `let _ = stream_write(...NULL...)` (never awaited → NULL is NEVER sent to a client whose claim failed), service.rs:1017 `let _ = tokio::spawn(...)` (benign). So the hypothesis of 'five sleeps' is refuted: it is 3 sleeps + 1 lost write + 1 benign.

**Fix:** Add `.await` to all four; use `tokio::time::interval`.

### C4. (c) CONFIRMED: shared `data` tuple races; lost failure results leave ghost entries in State.clients

**Severity:** high  
**Category:** concurrency  
**Location:** `src/service.rs:332`

Main loop holds `data.lock()` (332) for the whole iteration including `sleep(200ms)` (480). Each spawned task (397) writes `*data = (...)` (426) after its monitor finishes (up to 6s TCP / 3s HTTP). With >1 task in flight, task N+1 can overwrite task N's tuple before the main loop reads it (tokio Mutex is FIFO: queued writers run before the main loop re-locks). A task that saw failure does NOT push the hb back (475-478) so the entry leaves the deque, but if its tuple is overwritten `rmv()` never runs → payload stays in `clients` forever and remains claimable. Also the main loop always acts on the PREVIOUS iteration's result, and the 'reset service_claim' block (357-370) is unreachable-in-effect: it reads `shared_data.4` after it was zeroed at 355, and looks for `item.service_id == 0`, which never exists (seq starts at 1).

**Fix:** Replace the tuple with an `mpsc` channel of `MonitorResult` consumed by the main loop, or perform rmv inside the task under the State lock. Delete lines 357-370.

### C5. Any single failed heartbeat removes the entity; the 10-failure threshold is dead logic

**Severity:** high  
**Category:** logic  
**Location:** `src/service.rs:408`

`fail_id = hb.service_id` whenever status != OK and != ACCEPTED (408-411, also on Err 417-420). `service_id` is never 0 (add(): `temp_id = self.seq` ≥1, or the claimed service's id, 600-610). Then `data.0 < 10 && data.4 == 0` is false → hb dropped (475-478) and main loop `shared_data.4 != 0` (335) calls `rmv`. `FailCounter::increment` (98-104) with its 5s `interval`, `first_increment`, and the `fail_count == 10` check are therefore never the deciding factor.

**Fix:** Make fail_id only signal a *confirmed dead* entity (fail_count>=N); treat transient errors as increments.

### C6. (d) CONFIRMED: REST server branch in api.rs is unreachable; api_builder.rs is dead code

**Severity:** high  
**Category:** dead-code  
**Location:** `src/api.rs:63`

cli.rs:17-31 declares positional `operation` with `.required(true)`. `init()` calls `.get_matches()` (cli.rs:136), which on a missing required arg prints usage and `process::exit(2)`. Therefore `matches.contains_id("operation")` (api.rs:63; clap_builder try_contains_id = `self.args.contains_key(id)`) is always true, the `else` at api.rs:112 never runs, and `handle_requests` (api.rs:131) plus all of `src/api_builder.rs` (640 lines) are unreachable.

**Fix:** Either add a `serve` operation / `--rest` flag, or use a clap subcommand structure; delete or wire up api_builder.

### C7. (e) CONFIRMED: Dockerfile CMD, test script and doc examples use a removed CLI

**Severity:** medium  
**Category:** ops  
**Location:** `Dockerfile:77`

Dockerfile CMD: `-n en0 --ip-version 4 --operation listen --bind-port 12000 --tls`. `--operation` is not a defined arg (cli.rs has positional OPERATION) → clap error 'unexpected argument'. Also `en0` is a macOS interface; in alpine it is `eth0` → `only_or_error` panic (utils.rs:7). `--tls` needs CERT_PATH/KEY_PATH (tls.rs:22,34) but nothing loads `.env` (no dotenv crate; compose.yaml has no env_file). `src/test_event_monitor.sh` uses `./target/debug/nsm` (no such bin), `-o list_ips`, `--operation`, `--host X --port Y` (no --port flag). models.rs doc examples (lines 9, 25, 45, 72, 113, 154, 182) same.

**Fix:** Rewrite CMD as `["listen", "-n", "eth0", ...]`; move test script to tests/; fix docs.

### C8. (f) CONFIRMED: stream_read framing is 'short read = end of message' → panics, hangs, and coalescing bugs

**Severity:** high  
**Category:** protocol  
**Location:** `src/connection.rs:225`

Loop breaks when `bytes_read < 1024` (225). Consequences: (1) message length exactly k*1024 → extra read blocks 6s → TimedOut (220). (2) Two JSON messages coalesced in one segment (broker `receive()` writes ACK at 271 then request_handler CLAIM writes a 2nd ACK at service.rs:821 on the same stream) → `deserialize_message` `unwrap` (195) panics on 'trailing characters'. (3) EOF returns `Ok("")` → `serde_json::from_str("")` panics. (4) `std::str::from_utf8(..).unwrap()` (222) panics on non-UTF8 or a multibyte char straddling a 1024 boundary. (5) `stream_write` uses `write()` not `write_all()` (204-208) → partial writes of large PUB payloads (root_ca base64 chain is several KB).

**Fix:** Length-prefix frames (u32 BE + JSON) or newline-delimited JSON with `BufReader::read_line`; use `write_all`; return errors instead of unwrap.

### C9. Broker accepts connections serially and panics on any malformed/unexpected message

**Severity:** critical  
**Category:** robustness  
**Location:** `src/connection.rs:351`

`tcp_server` does `let _ = handler(Some(shared_stream)).await;` inside the accept loop (351) - no spawn. `request_handler` panics on: `receive(..).unwrap()` (service.rs:736), `collect_request(..).unwrap()` (737), `panic!` for ACK/COL/NULL headers (745,748,750), `deserialize(..)` unwrap (746-747, 719-721), `serde_json::from_str(..).unwrap()` for MSG/HB (877, 929), `only_or_error(&p.service_addr)` in add (514). Also CLAIM path sleeps 5×300ms (839) and MSG/HB paths loop 50×300ms / 50×702ms (922, 984) while blocking the accept loop.

**Fix:** `tokio::spawn` per connection; replace all unwrap/panic in the request path with error responses; add fuzz tests for the message parser.

### C10. (g) CONFIRMED: Heartbeat.tls derived from root_ca presence, not from transport

**Severity:** medium  
**Category:** logic  
**Location:** `src/service.rs:548`

`let tls = match p.root_ca { Some(_) => true, None => false }` (548-551). Used in `monitor()` to pick `https://` vs `http://` (216-229). A peer that relies on native roots (no `--root_ca`) is heartbeated over plain HTTP; a TCP-mode peer that passes `--root_ca` gets tls=true (unused).

**Fix:** Carry `Transport` in Payload/Heartbeat (Addr already has it); drop the bool.

### C11. (h) CONFIRMED with detail: ping-mode client refreshes the SERVICE's heartbeat, not its own → removed after 60s

**Severity:** high  
**Category:** logic  
**Location:** `src/service.rs:1078`

`ping_heartbeat` derives `id`/`service_id` from `payload` (1078-1081). For a client (operations.rs:593-597), `service_payload` is the SERVICE's payload from the ACK, so pings carry the service's `id`. The broker HB path matches `e.service_id == hb_body.service_id && e.id == hb_body.id` (944) → updates the service's `last_increment` (961), never the client's. The client's own hb hits `last_increment > 60s` (279) → GONE → fail_id → `rmv` → returns CLAIM and resets the service's `service_claim = 0` (663-671). Additionally, in ping mode `monitor()` never sends anything (143-277) so `msg_body` set by MSG is never delivered.

**Fix:** Store the client's own (id, service_id) from the broker's ACK in a separate Arc and use it for pings; make ping+MSG delivery explicit (poll endpoint).

### C12. (i) CONFIRMED: State::add reconnect loop can spin forever holding the State lock

**Severity:** critical  
**Category:** concurrency  
**Location:** `src/service.rs:560`

`while counter < 10` (560) increments `counter` only inside `find_map` for NON-matching deque elements (569). If the deque is empty, or the only element IS the match but `first_increment` is ≥60s old (574 false → no return, no increment), the loop never terminates. The caller (`request_handler` PUB, 762) holds `lock.lock()` for the duration, so `event_monitor` (311/336/391) blocks forever. `first_increment` is only refreshed on some failure branches (197,240,264), so for a healthy long-lived entity it equals registration time. Also `counter` can jump past 10 (e.g. 3 elements → 12) so the `counter == 10` warning (594) is skipped.

**Fix:** Bound the loop by attempts (not element count), release the lock between attempts, and handle the '>60s' case by falling through to create a new entry.

### C13. (j) CONFIRMED: handle_publish/handle_claim return 200 before the operation runs; spawned op never completes

**Severity:** medium  
**Category:** api  
**Location:** `src/api_builder.rs:218`

`tokio::spawn(async move { ... publish(...).await ... let _ = Ok(task_response); })` (218-246) then unconditionally `*response.body_mut() = "Successful request to publish"` (248). Same in handle_claim (350-380). `publish(…, API)` never returns (bind-port accept loop, mode_api/operations.rs:308) so the JoinHandle result would be unusable anyway; a 2nd request for the same bind_port panics on bind (`?` at mode_api:286 / `.unwrap()` operations.rs:582) inside the detached task. Also bool params: `data.get("ping").map_or(false, |v| v == "true")` (231, 362) compares `serde_json::Value` to a &str → JSON `true` (boolean) yields false; only the string "true" works. `print_v6` default differs: true in publish (222) vs false in claim (354).

**Fix:** Return 202 with a task id, or restructure so registration is synchronous and only the heartbeat server is detached; parse bools with `as_bool()`.

### C14. (k) CONFIRMED: handle_collect overwrites error body with 'Successful' while leaving status 500

**Severity:** low  
**Category:** api  
**Location:** `src/api_builder.rs:503`

Lines 482-501 set body/status based on `collect()`; line 503-505 then unconditionally sets body to `format!("Successful request to collect: {:?}", result)` where `result` is `()` (the match arms return unit). On error the response is `500` with body 'Successful request to collect: ()'. `collect()` also `panic!`s on 4 paths (operations.rs:686, 765, 772, 773) which kills the connection task with no response.

**Fix:** Return the collected message in the body; remove the overwrite; make collect() return Result.

### C15. (l) CONFIRMED: FailCounter.first_increment updated only in 3 of 7 failure branches; never on TCP path

**Severity:** low  
**Category:** logic  
**Location:** `src/service.rs:177`

Set at 197-199 (generic Err), 240-242 (HTTP Err), 264-266 (empty response). NOT set for ConnectionReset (177-182), ConnectionAborted (183-188), TimedOut (189-194), or HTTP timeout (251-254, which also doesn't increment at all). `stream_read` maps its timeout to `ErrorKind::TimedOut` (connection.rs:220) so the TCP generic `Err(_err)` branch (195) with comment 'read timed out' is effectively unreachable → `first_increment` is never updated on the TCP path. Its only consumer is the `<60s` reconnect window in `add()` (574).

**Fix:** Move first_increment logic into `FailCounter::increment()`; or delete the field (see finding on dead threshold).

### C16. (m) REFUTED as stated; actual bug: crypto-provider features are empty so install_default() never compiles, and `--features ring` cannot build

**Severity:** medium  
**Category:** build  
**Location:** `Cargo.toml:32`

`[features] ring = []` and `aws-lc-rs = []` (Cargo.toml:32-34) do not forward to `rustls/ring` or `rustls/aws_lc_rs`. All `install_default()` calls (operations.rs:651-654, 785-788; tls.rs:81-84) are behind `#[cfg(feature = ...)]` and are compiled OUT by default - so they are not called 'each call', they are never called. TLS works only because `hyper-rustls` default features pull `rustls/aws_lc_rs` (cargo tree confirms; `ring` is absent from the graph), so rustls auto-installs from crate features. Building with `--features ring` would fail: `rustls::crypto::ring` module does not exist without `rustls/ring`.

**Fix:** `ring = ["rustls/ring", "hyper-rustls/ring", "tokio-rustls/ring"]` etc.; call `install_default()` once in `main`; make `rustls` default-features explicit.

### C17. (n) CONFIRMED (low impact): runtime config inconsistent between binaries

**Severity:** info  
**Category:** consistency  
**Location:** `src/tcp.rs:41`

`#[tokio::main(flavor = "multi_thread", worker_threads = 20)]` in tcp.rs:41 vs plain `#[tokio::main]` in api.rs:53. Not a correctness bug, but the broker's throughput is dominated by the 200ms sleep per event (service.rs:480), not thread count. Both binaries call `std::process::exit` from inside tasks (8 sites) which skips destructors and in-flight writes.

**Fix:** Single binary with a shared runtime config; replace process::exit with cancellation tokens.

### C18. (o) CONFIRMED: Addr parsing/formatting inconsistencies; bare host:port panics in API mode

**Severity:** medium  
**Category:** parsing  
**Location:** `src/connection.rs:96`

`Addr::from_str` requires a numeric last `:` segment (114), so `http://host` (no port) is rejected - documented. But: (1) SOCKET-transport `Addr::to_string()` yields `host:port` (77); API-mode code then does `url::Url::parse(&inputs.host.to_string()).unwrap()` (operations.rs:435) / `?` (mode_api/operations.rs:181): `10.0.0.1:12000` fails (`RelativeUrlWithoutBase`) → panic; `localhost:8080` parses with scheme `localhost` and `.join("request_handler")` gives `localhost:request_handler` → hyper error. (2) IPv6: `[::1]:8080` works for `connect()` (200, std accepts brackets) but `to_socket_tuple()` (54-57) returns `("[::1]", 8080)` which does not resolve; `::1:8080` works for the tuple but `format!("{}:{}")` in `connect`/`tcp_server` (200, 341) fails to parse. (3) Port is `i32`, negative or >65535 accepted at parse time and only rejected later (58-61) or at bind.

**Fix:** Store `Transport`, `host: url::Host`, `port: u16`; validate in from_str; add unit tests for the listed strings.

### C19. send/MSG delivery never matches a heartbeat: relayed MsgBody.id is always 0

**Severity:** high  
**Category:** logic  
**Location:** `src/service.rs:890`

Peer `heartbeat_handler` relays MSG with `MsgBody{ id: 0, service_id }` (1263-1267 TCP, 1296-1300 API). Broker MSG path searches `e.id == msg_body.id` (890-891); heartbeat ids start at 1 (`seq: 1`, 505) → never found → 50 × 300ms = 15s spin (885-923) blocking the accept loop, then 'giving up'. Message never stored, never delivered.

**Fix:** Match on `e.id == msg_body.service_id` (the service's hb) or populate id correctly; add an integration test send→collect.

### C20. TCP peer never replies to send_msg; sender times out after 6s

**Severity:** medium  
**Category:** protocol  
**Location:** `src/service.rs:1253`

In `heartbeat_handler` MSG/TCP branch (1253-1273) the peer relays to the broker but writes nothing back on the incoming `stream`. `send_msg` uses `send()` which does `stream_read` after writing (connection.rs:247) → 6s TimedOut → 'Failed to collect message.' (operations.rs:815). Then the peer's per-connection loop (1019-1026) reads the now-closed socket → EOF → `""` → `deserialize_message` panic in the spawned task (1213).

**Fix:** Write an ACK on the MSG branch; make the per-connection loop exit on EOF instead of panicking.

### C21. Two-sided API heartbeats panic the broker's monitor task; entity vanishes from deque but not from State

**Severity:** high  
**Category:** robustness  
**Location:** `src/service.rs:236`

API-mode peers (operations.rs:530-549 claim, mode_api/operations.rs:238-255 publish) serve a stub returning 400 'Not implemented' (plain text) for every path. `api_server` routes GET /heartbeat_handler to it (connection.rs:314-319). Broker `monitor()` does `collect_request(resp.body_mut()).await.unwrap()` (236) → serde error on non-JSON → panic in the spawned task (397). The task dies before pushing hb back (471) or writing `data` (426) → hb gone from deque, payload remains in `clients` forever.

**Fix:** Wire `heartbeat_handler_helper` back into the API peer handlers; treat non-JSON responses as failures; never unwrap in monitor.

### C22. Peer heartbeat_handler calls process::exit on any 6s idle read; broker cycle time scales with peer count

**Severity:** high  
**Category:** liveness  
**Location:** `src/service.rs:1223`

`heartbeat_handler` TCP path exits the process on TimedOut/Reset/Aborted/other (1215-1230). `stream_read` times out after 6s (connection.rs:215). The broker sends one HB per deque entry per cycle, with `sleep(200ms)` per event (480) and the entry re-enqueued only after `monitor()` completes (up to 6s). Cycle time ≈ max(N×200ms, monitor latency). Any event-loop stall (State lock held during PUB `add()` connect attempts at 762-766, or the CLAIM sleep at 839 while holding `state_loc`) adds directly.

**Fix:** Decouple: per-entity heartbeat task with its own interval; peers should tolerate missed beats with a counter, not exit.

### C23. State::claim treats any payload under the key as a service and re-issues claimed services after 60s

**Severity:** high  
**Category:** logic  
**Location:** `src/service.rs:689`

`claim()` iterates every Payload in `clients[k]` (689) - both services (service_id == id) and CLIENT payloads (service_port -1, pushed by add() at 631) - and returns the first with `epoch - service_claim > timeout(60)` (691). No `service_id == id` filter, and no 'currently claimed' state: a claimed service becomes available again 60s later regardless of client liveness. Client payloads carry `service_claim: epoch()` (operations.rs:324) so they too become 'claimable' after 60s.

**Fix:** Filter `v.service_id == v.id && v.service_port > 0`; track claim ownership (client id) and free only on client removal.

### C24. State lock held across network I/O and sleeps in request_handler

**Severity:** medium  
**Category:** concurrency  
**Location:** `src/service.rs:762`

PUB: `state_loc = lock.lock()` (762) held while `add()` performs `TcpStream::connect` ×5 (523) and `setup_https_client` (543). CLAIM: guard taken at 806 lives across `sleep(300ms)` (839) for 5 retries. `event_monitor` needs the same lock at 311/336/359/382/391 each iteration.

**Fix:** Do network work outside the lock; build the Heartbeat first, then lock briefly to insert.

### C25. event_monitor deep-clones State.clients under lock on every event

**Severity:** low  
**Category:** performance  
**Location:** `src/service.rs:392`

`state_loc.clone()` (392) clones the whole `HashMap<u64, Vec<Payload>>` (Payload has 3 Vec<String>/Option<String> incl. base64 CA chains) every 200ms per event, while holding the State lock. O(N) per heartbeat → O(N²) per cycle. The clone is only used for the broken re-claim path.

**Fix:** Remove the clone (fixing (a) removes the need).

### C26. Single-slot GLOBAL_MSGBODY loses messages; broker re-sends the same msg on every HB

**Severity:** low  
**Category:** logic  
**Location:** `src/service.rs:69`

Peer stores the last HB-carried message in one global (1464-1465). Broker keeps `hb.msg_body` set forever (898) and includes it in every heartbeat (151), so the peer overwrites the slot each cycle with the same message; a second MSG replaces the first before `collect` runs. No consumed/ack semantics.

**Fix:** Queue messages per service; clear on delivery ack.

### C27. Request/response body parsing panics on malformed input in API mode (per-connection)

**Severity:** medium  
**Category:** robustness  
**Location:** `src/connection.rs:283`

`collect_request`: `request.collect().await.unwrap()` (283) and `deserialize_message(&json)` unwrap (292/195) → panic on body read error or JSON not shaped like Message. Callers add more unwraps: service.rs:737, 1233, 1344-1346, 1154-1159; operations.rs:498, 760, 882; mode_api/operations.rs:205. In API mode each connection is spawned (mode_api:102) so the blast radius is one connection, but on the client side (operations.rs:498/760/882) it aborts the CLI.

**Fix:** Return `Result` and map to 400; use `serde_json::from_slice::<Message>` directly (the Value round-trip at 285-292 is redundant).

### C28. only_or_error panics whenever the interface has ≠1 matching address

**Severity:** medium  
**Category:** robustness  
**Location:** `src/utils.rs:7`

`only_or_error` panics 'Vector does not contain a single element' (7). Called in listen (operations.rs:182), publish (261), claim (318), and broker-side `add()` on the REMOTE payload (service.rs:514). `get_matching_ipstr` with `-n` omitted matches all interfaces (network.rs:98), and `--ip-version` omitted sets both v4 and v6 (cli.rs:173-174) but only v4 is used when `print_v4` is true.

**Fix:** Return Result; pick a deterministic address or require `--ip-start`; validate remote payloads.

### C29. cli.rs uses assert!/unwrap for missing optional flags; `collect`/`send` require flags they don't use

**Severity:** low  
**Category:** cli  
**Location:** `src/cli.rs:209`

`assert!(args.contains_id("bind_port"))` (209, 230-232, 261-264, 295, 320-321) panics with an internal assertion message instead of a usage error. `collect` requires `--key` via `unwrap()` (302) though `Collect.key` is never read by `collect()`; `send` unwraps `--msg` (329). `--tls` is parsed for every op (177) though TCP mode ignores it (mode_tcp never reads tls).

**Fix:** Use clap subcommands with per-subcommand required args (derive API).

### C30. GLOBAL_LAST_HEARTBEAT watchdog is refreshed by ANY GET /heartbeat_handler, including collect requests

**Severity:** low  
**Category:** logic  
**Location:** `src/connection.rs:314`

`api_server` sets `GLOBAL_LAST_HEARTBEAT = now` (315-318) before dispatching any GET whose path starts with /heartbeat_handler. `collect()` API mode sends GET /heartbeat_handler (operations.rs:731-735). The watchdog (operations.rs:562-579, mode_api:262-279) therefore cannot distinguish broker heartbeats from user polls. It also never fires if the broker never sent a first heartbeat (`None => continue`, 571).

**Fix:** Set the timestamp inside the HB handler only after validating the message header.

### C31. mode_tcp listen swallows accept-loop panics/bind failures; broker appears healthy while deaf

**Severity:** medium  
**Category:** robustness  
**Location:** `src/mode_tcp/operations.rs:40`

`let _thread_handler = tokio::spawn(tcp_server(...))` (40-42) - JoinHandle dropped; `TcpListener::bind(...).unwrap()` (connection.rs:342) panics if the port is busy; any handler panic (see other findings) also kills this task. `listen` only awaits `event_loop` (53), which runs forever.

**Fix:** `tokio::select!` on both handles; propagate errors; exit non-zero on accept-loop death.

### C32. heartbeat_handler_helper spawns a heartbeat loop for EVERY inbound connection, including collect/send

**Severity:** medium  
**Category:** logic  
**Location:** `src/service.rs:1017`

TCP branch (1016-1029) spawns `loop { heartbeat_handler(...) }` per accepted stream. For a `collect` or `send` connection, after answering once the loop reads again: if the CLI keeps the socket open >6s → `process::exit(0)` (1223-1225); if it closes → EOF → `""` → deserialize panic (1213). The helper also returns an empty `Response` immediately (1028) so `tcp_server`'s `let _ = handler(...).await` learns nothing.

**Fix:** Distinguish the broker's heartbeat stream from one-shot request connections; handle one message per connection for COL/MSG.

### C33. Unused / misconfigured dependencies

**Severity:** low  
**Category:** dependencies  
**Location:** `Cargo.toml:15`

`threadpool = "1.8"` (15) - no `use threadpool` anywhere (only in comments service.rs:396,484); `rustls-platform-verifier` (22) optional, never enabled, yet 2 crates vendored (vendor/rustls-platform-verifier{,-android}); vendor/ contains both `rustls-native-certs` 0.8.1 and 0.7.3; `lazy_static` (16) can be `std::sync::LazyLock` on rustc 1.98; `hyper-util` legacy `Client` (flagged by TODOs tls.rs:9, mode_api:26); `rustls = { default-features = false }` while relying on hyper-rustls defaults to bring the provider. `cargo tree -i rustls-platform-verifier` → 'did not match any packages'.

**Fix:** Remove threadpool, rustls-platform-verifier, lazy_static; `cargo vendor` afresh (or stop vendoring); pin provider explicitly.

### C34. Dead code inventory

**Severity:** info  
**Category:** dead-code  
**Location:** `src/service.rs:357`

(1) service.rs:357-370 'reset service_claim' block: always no-op (see (c)). (2) `State::new(_tls: Option<ClientConfig>)` param unused (501). (3) `Payload.interface_addr` written (operations.rs:252) never read. (4) operations.rs:446-450 `_server_name` computed and discarded. (5) `HttpResult` return of listen/publish/claim is always `Response::new(Full::default())` (205, 280, 643). (6) `#[allow(dead_code)]` on `State::claim` (684) although used. (7) `FailCounter.interval/first_increment` effectively dead (see threshold finding). (8) `Heartbeat.tls` (124) wrong source. (9) `utils::only_or_none` (utils.rs:13) unused. (10) `mode_api::get_https_connector` (132-157) duplicates `tls::setup_https_client` (tls.rs:105-141). (11) Commented-out code: service.rs:1351-1357, api.rs:159-161, api_builder.rs:445-455, 634-640, operations.rs:535-541, mode_api:243-247. (12) `MessageHeader::NULL` reply never sent (service.rs:851). (13) `cli.rs` `.version("1.0")` vs Cargo 0.1.0. (14) All of `api_builder.rs` + `api.rs:131-168` unreachable. (15) Duplicate module trees declared in both binaries (tcp.rs:8-23, api.rs:8-25) instead of a lib.rs.

**Fix:** Create `src/lib.rs`, delete unreachable code, run `cargo +nightly udeps`/`cargo machete`, enable `#![deny(dead_code)]` after cleanup.

### C35. Repository hygiene: tracked secrets/paths, binaries, stale docs, .gitignore typos

**Severity:** low  
**Category:** repo  
**Location:** `.gitignore:6`

`.gitignore` lines 6 and 9 read `view-events-rolebinding.yam` and `server.keyl` (typos) → `view-events-rolebinding.yaml` IS tracked and a future `server.key` would not be ignored. Tracked: `.env` (another developer's absolute cert paths), `nsm-dev-buildx-latest.tar` (18MB Docker image), `src/.DS_Store` (despite commit 49b35474 'dont need DS_Store'), `docs/` (10,971 files of 2024 rustdoc for a binary `nsm` that no longer exists), `vendor/` (12,449 files). `.git` is 340MB.

**Fix:** Fix typos; `git rm --cached` the artifacts; regenerate docs in CI (gh-pages) instead of tracking; consider dropping vendoring or using a lockfile-only offline strategy.


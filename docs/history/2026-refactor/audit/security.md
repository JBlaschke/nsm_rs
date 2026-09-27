# Audit: Security

Findings reference `main` at commit `edd23a33` (2026-09-24), the state before the cleanup branches. Line numbers will drift as the cleanup lands; the file names are stable enough to locate each item.

Severity counts: 4 critical, 14 high, 12 medium, 10 low, 0 info. Each finding in this lens was independently re-checked by three reviewers (code reading, impact, compensating controls); none was refuted, and severities below are the reviewers' consensus.

## Summary

SECURITY lens over /Users/johannes.blaschke/Developer/nsm_rs (crate nsm, bins `tcp` and `api`). Verified against source; no files modified.

Headline: the broker and every participant are trivially killable from the network, and there is no authentication anywhere.

- Remote crash of the TCP broker with one connection: `tcp_server` awaits the handler inline (connection.rs:351) inside one spawned task (mode_tcp/operations.rs:40); any panic in `request_handler` (`receive().unwrap()` 736, `deserialize_message` unwrap connection.rs:195 on empty/garbage JSON, `from_utf8().unwrap()` connection.rs:222 on a multibyte split, `only_or_error` service.rs:514, ACK/COL/NULL header panics 745-750/990, `from_str::<MsgBody>().unwrap()` 877/929) unwinds through the accept loop and the broker stops accepting forever. A single malformed or empty connection is enough.
- Remote kill of every service/client: `heartbeat_handler` calls `std::process::exit(0)` on any read error (service.rs:1217-1229); anyone who connects to a party's bind_port and idles 6 s terminates that process. In the `api` binary the GLOBAL_LAST_HEARTBEAT watchdog (operations.rs:576, mode_api/operations.rs:276) does the same to the whole REST server ~15 s after any unauthenticated POST /publish or GET /claim.
- REST control plane api.rs:115 binds 0.0.0.0:8080, no auth: SSRF to arbitrary `host`, binds arbitrary local `bind_port`, spawns unbounded long-lived tasks, and `root_ca` is a server-side file path whose PEM certificates are read and shipped base64 to the attacker-chosen host (operations.rs:232-242, 303-313) = file exfiltration primitive.
- Authorization is a guessable/unsecret u64 `key` sent in cleartext (TCP mode has no TLS at all): any peer can PUB a rogue service under a victim key (service hijack), CLAIM to learn victim service address/all interface IPs/CA (service.rs:807-815), and MSG to overwrite any Heartbeat.msg_body by sequential id (877-898). Parties' bind_ports also accept COL/HB/MSG from anyone (1384-1486).
- TLS is decorative: `https_or_http()` everywhere, `with_no_client_auth()` everywhere, trust anchors are supplied by the peer itself inside Payload.root_ca (tls.rs:107-123, service.rs:543), native-roots fallback, broker talks `http://` to any party without root_ca (service.rs:225, 548-551).
- DoS: CLAIM holds the State lock across 5x300 ms sleeps (service.rs:806-840); `State::add` connects back to attacker-chosen addr under the same lock (523-535, retry sleep not awaited at 532); MSG/HB loops hold a TCP connection 15 s/35 s while the accept loop is serialized; unbounded body/stream reads and unbounded `State.clients`.
- Framing: `bytes_read < 1024` as end-of-message (connection.rs:225) is wrong for fragmentation/coalescing and for real Payloads carrying root_ca (>1 KiB) -> truncated JSON -> panic; `write()` not `write_all()` (204-209).
- Repo/ops hygiene: .env with another developer's cert/key paths committed; .gitignore typos (`server.keyl`, `.yam`) mean `server.key` is not ignored; Dockerfile CMD uses undefined `--operation` flag and macOS `en0`, no ca-certificates, alpine 3.18 EOL, CERT_PATH/KEY_PATH unset; 18 MB image tarball and 156 MB stale docs tracked; rustls provider feature plumbing is inert (empty `ring`/`aws-lc-rs` features), provider comes only from hyper-rustls defaults.


## Detail

## Remote-triggerable panic / exit inventory (all reachable by an unauthenticated peer)

| Site | Trigger | Effect |
|---|---|---|
| connection.rs:222 `from_utf8().unwrap()` | non-UTF-8 or split multibyte | panic; TCP broker acceptor dies |
| connection.rs:195 `deserialize_message` | EOF ("" ), non-Message JSON | panic (broker 736/262, party 1213, claim client 372, send 248) |
| connection.rs:283/292 `collect_request` | aborted body, non-Message JSON | panic (broker API 737, monitor 236, party 1233, clients) |
| connection.rs:342 `bind().unwrap()` | port in use / bad port | panic at start of tcp_server |
| service.rs:514 `only_or_error` | service_addr len != 1 | panic under State lock |
| service.rs:745/748/750/990 | header ACK/COL/NULL | panic |
| service.rs:877, 929, 1438, 1465 `from_str::<MsgBody>().unwrap()` | malformed body | panic |
| service.rs:236 monitor `collect_request().unwrap()` | party answers non-JSON (default stub does) | monitor task dies, stale Payload persists |
| service.rs:258, 738, 793, 831, 865, 1234, 1382, 1409, 1433, 1460, 1484 `panic!("Unexpected state")` | internal | defensive, should be types |
| service.rs:1217-1229 `process::exit(0)` | idle/closed inbound connection on bind_port (6 s) | party process exits |
| service.rs:1171, 1179 `process::exit(0)` | broker unreachable 10x | pinging party exits |
| operations.rs:576, mode_api/operations.rs:276 `process::exit(0)` | no GET /heartbeat_handler for 10 s | party exits; in `api` bin the whole REST server exits |
| operations.rs:509, 516, 522; mode_api/operations.rs:224, 230; service.rs:1363, 1369 `panic!` | broker not answering | task/CLI dies |
| operations.rs:686, 765, 772, 773 collect `panic!` | empty/invalid response | CLI dies / REST conn task dies |
| tls.rs:111 base64 unwrap | garbage root_ca | panic under State lock |
| tls.rs:22/34 `expect`, 25/37/53 `unwrap`, 65/68 native certs unwrap, 87/88/94 | env/files missing | startup or first-TLS-use panic (in spawned REST tasks: silent) |
| operations.rs:435/446/449/477 url unwraps | SOCKET-transport host through HTTP path | panic in spawned task |
| service.rs:343, mode_api/operations.rs:211-212 `parse().unwrap()` | non-numeric ACK body from broker | panic |
| service.rs:691 u64 underflow | wire service_claim > now | debug panic / release wrap |

## Lock-across-await map (State mutex `lock` in AMState)
- service.rs:806-840 CLAIM: guard alive across `sleep(300ms)` x5 -> HELD (bug).
- service.rs:762-794 PUB: guard held across `State::add` -> TcpStream::connect (TCP, up to 6 tries, no timeout) or `setup_https_client` (API) -> HELD across network I/O.
- service.rs:816-829 CLAIM success: guard held across `stream_write` + `State::add` connect-back -> HELD across network I/O.
- service.rs:886-921 MSG and 938-983 HB: guard scoped inside block, sleep outside -> OK, but nested `state_loc.deque.lock()` inside State lock = lock ordering State->deque everywhere; event_monitor takes deque without State (374, 458, 471) -> no deadlock today but fragile.
- service.rs:391-393 event_monitor: State lock held while cloning entire State.

## Attack surface summary
1. Broker listen port (TCP JSON or HTTP POST /request_handler): PUB, CLAIM, MSG, HB - no auth.
2. Every party's bind_port (TCP or HTTP GET /heartbeat_handler, POST /request_handler): HB, MSG, COL - no auth; process::exit on idle.
3. `api` binary 0.0.0.0:8080: /publish, /claim, /collect, /send, /list_interfaces, /list_ips - no auth; spawns tasks, binds ports, dials arbitrary hosts, reads server files (root_ca).
4. Outbound: broker dials every peer-declared service_addr:bind_port; parties dial the broker address given on the command line/REST.

## Suggested remediation branches (security-relevant ordering)
1. `sec/no-panic-no-exit`: replace every unwrap/panic/process::exit on network paths with Result; spawn per-connection in tcp_server; add `clippy::unwrap_used`, `clippy::expect_used` deny for src/. Tests: fuzz-ish unit tests feeding "", garbage, partial JSON, ACK/COL/NULL headers into request_handler/heartbeat_handler.
2. `sec/framing`: length-prefixed frames with cap, write_all, read_exact; `Limited` HTTP bodies; caps on State size. Tests: exact-1024, fragmented, coalesced frames.
3. `sec/locks-and-dos`: scope State guards, connect with timeout outside the lock, replace polling loops with Notify, connection semaphore + accept error handling, header/handshake timeouts.
4. `sec/authn-authz`: mTLS via ROOT_PATH-configured trust only; remove `root_ca` from Payload; bind PUB/CLAIM/MSG/COL to certificate identity; server-assigned ids/service_claim; separate services from clients in State; `https_only()`.
5. `sec/control-plane`: bind REST to loopback/configured addr, token/mTLS auth, destination allow-list, remove file-path parameters, task registry with status endpoint, exact routes, remove interface enumeration.
6. `sec/logging-secrets`: delete println!/full-request logging, redacted Debug for Payload; remove .env, fix .gitignore, remove tarball/docs/.DS_Store from git; fix Dockerfile (positional op, eth0, base image, cert secrets), dedicated k8s ServiceAccount.
7. `deps/bump`: pin a single rustls provider explicitly, drop threadpool/rustls-platform-verifier-or-enable-it, bump hyper/hyper-util/rustls/tokio/pnet, run `cargo audit`/`cargo deny` in CI.

## Verified non-issues / notes
- Docker image tarball config contains only PATH in Env (no embedded secrets), user appuser.
- No private keys tracked in the repo outside vendored crates' test fixtures (vendor/schannel/test/key.pem, vendor/tokio-rustls/tests/certs/end.key) which may trip secret scanners.
- tokio::sync::Mutex guards are released on unwind, so panics do not poison the State lock, but they do abort half-completed add/rmv sequences.
- hyper-rustls verifies hostname/IP from the request URI, so the discarded ServerName at operations.rs:446 does not by itself disable verification.

## Findings

Ids are `S` plus the finding number, in the order the reviewer reported them (not by severity).

| # | Severity | Category | Finding | Location |
|---|---|---|---|---|
| S1 | critical | remote-panic | Remote panic in stream_read: from_utf8().unwrap() on 1024-byte chunk | `src/connection.rs:222` |
| S2 | critical | remote-panic | deserialize_message unwraps attacker JSON; EOF yields empty string and panics | `src/connection.rs:195` |
| S3 | high | remote-abort | heartbeat_handler calls process::exit(0) on any read error: idle connection kills any service or client | `src/service.rs:1217` |
| S4 | high | remote-abort | api binary: unauthenticated /publish or /claim arms a watchdog that exits the whole REST server | `src/operations.rs:576` |
| S5 | critical | authz | REST control plane on 0.0.0.0:8080 with no authentication: SSRF, port squatting, unbounded task spawn | `src/api.rs:115` |
| S6 | critical | authz | No authentication of peers: u64 key is the only credential; any peer can PUB/CLAIM/MSG any key over cleartext TCP | `src/service.rs:807` |
| S7 | high | authz | MSG lets any peer overwrite any Heartbeat.msg_body by guessable sequential id | `src/service.rs:877` |
| S8 | high | remote-panic | only_or_error panics on attacker Payload with service_addr length != 1 (under State lock) | `src/service.rs:514` |
| S9 | medium | remote-panic | base64 decode unwrap on peer-supplied root_ca (API-mode PUB/CLAIM) | `src/tls.rs:111` |
| S10 | high | ssrf | Broker connects back to attacker-chosen address on every PUB/CLAIM (SSRF/scan amplifier) while holding State lock | `src/service.rs:523` |
| S11 | high | dos | CLAIM loop holds State mutex across sleep (1.5 s per unknown-key claim) - trivial broker stall | `src/service.rs:806` |
| S12 | high | dos | TCP server awaits handler inline: head-of-line blocking and single-panic acceptor death | `src/connection.rs:351` |
| S13 | high | protocol | Message framing uses bytes_read < 1024 as end-of-message; breaks on fragmentation, coalescing, exact multiples | `src/connection.rs:225` |
| S14 | high | dos | Unbounded reads and unbounded state growth (memory DoS) | `src/connection.rs:216` |
| S15 | high | remote-panic | Heartbeat::monitor unwraps collect_request on party response; non-JSON body evicts entry silently and leaves stale Payload | `src/service.rs:236` |
| S16 | medium | remote-panic | collect_request unwraps body collection; client aborting mid-body panics handler | `src/connection.rs:283` |
| S17 | high | remote-panic | request_handler panics on ACK/COL/NULL headers and unreachable arm | `src/service.rs:745` |
| S18 | high | remote-panic | serde_json::from_str::<MsgBody>().unwrap() on wire bodies in four places | `src/service.rs:929` |
| S19 | high | authz | Parties' bind_port endpoints are unauthenticated: COL leaks service payload/messages, HB injects messages, MSG triggers relay to broker | `src/service.rs:1384` |
| S20 | medium | tls | https_or_http() everywhere allows plaintext downgrade; broker uses http:// to any party without root_ca | `src/tls.rs:126` |
| S21 | high | tls | No client auth anywhere and trust anchors are supplied by the peer being verified | `src/tls.rs:92` |
| S22 | low | tls | Native root store fallback: any public CA can impersonate broker/parties; panics in minimal containers | `src/tls.rs:65` |
| S23 | low | tls | ALPN list contains invalid 'http/1.0'; server advertises h2 while every client is http1-only | `src/tls.rs:96` |
| S24 | low | tls | ServerName computed and discarded (dead TLS code) | `src/operations.rs:446` |
| S25 | low | logging | Full requests, payloads and broker state logged/printed (information disclosure) | `src/api_builder.rs:20` |
| S26 | medium | secrets | .env with another developer's certificate and key paths is committed | `.env:1` |
| S27 | medium | secrets | .gitignore typos mean server.key and role-binding files are NOT ignored | `.gitignore:7` |
| S28 | medium | deployment | Dockerfile CMD is broken and requires TLS env that is never provided | `Dockerfile:73` |
| S29 | low | repo-hygiene | 18 MB Docker image tarball and 156 MB stale rustdoc tracked in git | `nsm-dev-buildx-latest.tar:1` |
| S30 | low | deployment | Kubernetes RoleBinding grants role to the namespace default ServiceAccount | `view-events-rolebinding.yaml:7` |
| S31 | low | integer | Integer truncation and sign wrap in REST parameter parsing | `src/api_builder.rs:172` |
| S32 | medium | integer | u64 underflow in State::claim with wire-controlled service_claim; overflow-checks off in release | `src/service.rs:691` |
| S33 | medium | authz | State::claim can hand out a client Payload as a service (address leak, wrong pairing) | `src/service.rs:689` |
| S34 | medium | dos | event_monitor shared fail tuple is racy and evicts on the first non-OK heartbeat | `src/service.rs:335` |
| S35 | low | authz | Anonymous GET /heartbeat_handler resets liveness watchdog; prefix routing | `src/connection.rs:314` |
| S36 | low | dependencies | rustls crypto-provider feature plumbing is inert; provider only present via hyper-rustls defaults | `Cargo.toml:33` |
| S37 | medium | dos | HTTP servers have no connection limits, timeouts or TLS handshake timeout | `src/mode_api/operations.rs:92` |
| S38 | medium | file-read | REST publish/claim root_ca is a server-side file path read and exfiltrated to attacker host | `src/operations.rs:232` |
| S39 | low | robustness | Unhandled panics/asserts in CLI and unawaited sleeps hide protocol bugs | `src/cli.rs:209` |
| S40 | medium | protocol | Party-side HTTP heartbeat endpoint is a stub returning 400, so HTTP two-sided mode cannot work and triggers broker-side panics | `src/mode_api/operations.rs:238` |

### S1. Remote panic in stream_read: from_utf8().unwrap() on 1024-byte chunk

**Severity:** critical  
**Category:** remote-panic  
**Location:** `src/connection.rs:222`

`let s = std::str::from_utf8(&buf[..bytes_read]).unwrap();` runs on every 1024-byte chunk. Any non-UTF-8 byte, or a multibyte UTF-8 character split across the 1024 boundary, panics. `stream_read` is reached from `receive()` (connection.rs:261) inside `request_handler` (service.rs:736), which in TCP mode is awaited inline by `tcp_server` (connection.rs:351) inside the single spawned accept task (src/mode_tcp/operations.rs:40). The panic unwinds the accept loop; the broker keeps running (event_monitor alive) but never accepts another connection.

**Fix:** Read raw bytes into a Vec<u8>, parse UTF-8/JSON once on the complete frame with `String::from_utf8(..)?`/`serde_json::from_slice(..)?`, return io::Error on failure. Spawn one task per accepted connection in `tcp_server` so a failing connection can never take down the acceptor. Add `#![deny(clippy::unwrap_used)]` for network paths.

### S2. deserialize_message unwraps attacker JSON; EOF yields empty string and panics

**Severity:** critical  
**Category:** remote-panic  
**Location:** `src/connection.rs:195`

`serde_json::from_str(payload).unwrap()` is applied to every wire message. `stream_read` returns `Ok("")` on EOF (bytes_read 0 < 1024, connection.rs:225), so a client that connects and closes produces a panic in `receive()` (connection.rs:262), `send()` (248), `heartbeat_handler` (service.rs:1213) and TCP `claim` (operations.rs:372). Same for any non-Message JSON. In the TCP broker this kills the accept loop (see finding on connection.rs:351); in a service/client it kills the heartbeat task for that connection.

**Fix:** Make `deserialize_message` return `Result<Message, serde_json::Error>`; treat EOF (0 bytes) as ConnectionAborted; reject unknown headers with an error response rather than panic.

### S3. heartbeat_handler calls process::exit(0) on any read error: idle connection kills any service or client

**Severity:** high  
**Category:** remote-abort  
**Location:** `src/service.rs:1217`

Lines 1215-1230: ConnectionReset, ConnectionAborted, TimedOut and any other error from `stream_read` all call `std::process::exit(0)`. In TCP mode every connection accepted on a party's bind_port gets its own infinite `heartbeat_handler` loop (service.rs:1017-1027, via `tcp_server` in mode_tcp/operations.rs:112 and operations.rs:430). `stream_read` has a 6 s timeout (connection.rs:215).

**Fix:** Return `Err` and let the caller drop that connection only. Track liveness per broker connection (the stream the broker opened), not per arbitrary inbound connection; decide shutdown in main with a documented policy and a non-zero exit code.

### S4. api binary: unauthenticated /publish or /claim arms a watchdog that exits the whole REST server

**Severity:** high  
**Category:** remote-abort  
**Location:** `src/operations.rs:576`

`claim` (operations.rs:557-580) and `mode_api::publish` (src/mode_api/operations.rs:257-280) spawn a task that calls `std::process::exit(0)` when `GLOBAL_LAST_HEARTBEAT` is older than 10 s. In the `api` binary these run inside the long-lived REST process (api.rs:115) because `handle_publish`/`handle_claim` `tokio::spawn` them (api_builder.rs:218, 350). `GLOBAL_LAST_HEARTBEAT` is a process-wide lazy_static (operations.rs:53-55), so one stale party kills every task in the process, and any anonymous `GET /heartbeat_handler` (connection.rs:314-318) keeps it alive.

**Fix:** Remove process::exit from library code; give each publish/claim its own liveness state passed by handle; make the REST server own task lifecycles (cancel tokens) and never exit on a per-task condition.

### S5. REST control plane on 0.0.0.0:8080 with no authentication: SSRF, port squatting, unbounded task spawn

**Severity:** critical  
**Category:** authz  
**Location:** `src/api.rs:115`

`"0.0.0.0:8080".parse()` with no auth, TLS, rate limit or allow-list. `POST /publish` and `GET /claim` (api_builder.rs:110-250, 253-381) take attacker `host` (any http/https/socket Addr), `bind_port`, `key`, `name`, `starting_octets` and spawn `publish`/`claim` tasks that (a) connect to `host` (SSRF into the HPC network, operations.rs:485-492, mode_api/operations.rs:192-199), (b) `TcpListener::bind` on the API host at the attacker's `bind_port` (operations.rs:582, mode_api/operations.rs:286), (c) run forever with retry loops, and (d) return "Successful request" regardless of outcome (api_builder.rs:248, 379). `GET /collect` and `POST /send` (api_builder.rs:384-632) make one GET/POST with attacker `msg` to arbitrary host:port (operations.rs:729-750, 853-874). `GET /list_interfaces` and `/list_ips` (api.rs:140-146) enumerate host interfaces and IPs. Routes match by `starts_with` (api.rs:140-157).

**Fix:** Bind to loopback or a configured address; require an auth token/mTLS on the control plane; allow-list destination hosts/ports; cap concurrent tasks and return task ids with a status endpoint; exact-match routes; remove interface enumeration or restrict it.

### S6. No authentication of peers: u64 key is the only credential; any peer can PUB/CLAIM/MSG any key over cleartext TCP

**Severity:** critical  
**Category:** authz  
**Location:** `src/service.rs:807`

`state_loc.claim(payload.key)` (807) and `state_loc.add(payload, 0, ..)` (766/785) trust the wire Payload completely. Key is a u64 chosen by users (cli.rs:94-101) and transmitted in JSON in the clear; the TCP transport (bin `tcp`) has no TLS path at all (mode_tcp/operations.rs). Exploits: (1) service hijack: attacker PUBs `{service_addr:[attacker], service_port, key: victim}`; `State::claim` returns the first unclaimed entry (685-696), so victims' clients are handed the attacker's address and connect their data plane to it. (2) discovery: attacker CLAIMs victim key and receives the full service Payload incl. `interface_addr` (all IPs of the service host) and `root_ca` (814). (3) eviction/squatting: floods of PUBs under a key so real services are never selected.

**Fix:** Introduce real identity: mTLS with per-party certificates (or at minimum a shared HMAC secret per key with nonces), authorize PUB by an allow-list bound to the certificate identity, and never return other parties' interface lists. Drop plaintext TCP or wrap it in TLS.

### S7. MSG lets any peer overwrite any Heartbeat.msg_body by guessable sequential id

**Severity:** high  
**Category:** authz  
**Location:** `src/service.rs:877`

`let msg_body: MsgBody = serde_json::from_str(&message.body).unwrap();` then the deque is searched for `e.id == msg_body.id` (890-891) and `hb.msg_body = msg_body.clone()` (898) with no key or ownership check. Ids are `State.seq` starting at 1 (service.rs:505, 632). The stored body is delivered to the service on the next two-sided HB (149-152, 165-171) and stored by the service into `GLOBAL_MSGBODY` (1464-1465) where `collect` reads it.

**Fix:** Bind messages to the authenticated sender; only the client that claimed service X may message X (check hb.service_id/claim ownership); use unguessable ids (random u128) as a defence-in-depth; replace unwrap with error response.

### S8. only_or_error panics on attacker Payload with service_addr length != 1 (under State lock)

**Severity:** high  
**Category:** remote-panic  
**Location:** `src/service.rs:514`

`State::add` calls `only_or_error(&p.service_addr)` (utils.rs:4-9 `panic!`) on the deserialized wire Payload. A PUB/CLAIM with `"service_addr": []` or two entries panics inside `request_handler` while `lock.lock().await` is held (service.rs:762/806). TCP mode: accept loop dies. API mode: connection task dies; tokio Mutex guard is released on unwind but the deque/state may be half-updated.

**Fix:** Validate Payload after deserialization (exactly one addr, parseable IpAddr, 1..=65535 ports, key policy) and return 400/NULL on failure; make `only_or_error` return Result.

### S9. base64 decode unwrap on peer-supplied root_ca (API-mode PUB/CLAIM)

**Severity:** medium  
**Category:** remote-panic  
**Location:** `src/tls.rs:111`

`setup_https_client` does `.map(|line| general_purpose::STANDARD.decode(line).unwrap())` on `Payload.root_ca` supplied over the wire; called from `State::add` (service.rs:543) under the State lock. `"root_ca":"!!!"` panics the request_handler task. `add_parsable_certificates` silently ignores garbage DER so no error surfaces otherwise.

**Fix:** Propagate decode errors; reject root_ca from peers entirely (see trust-anchor finding) or bound its size and validate it parses as X.509 with a CA basic constraint.

### S10. Broker connects back to attacker-chosen address on every PUB/CLAIM (SSRF/scan amplifier) while holding State lock

**Severity:** high  
**Category:** ssrf  
**Location:** `src/service.rs:523`

`TcpStream::connect(bind_address)` with `bind_address = format!("{}:{}", ipstr, p.bind_port)` (515) from the wire Payload, retried 6 times (527); the retry `sleep` at 532 is not awaited so retries are immediate, but each connect to a filtered/unroutable host blocks for the OS SYN timeout (tens of seconds) while `request_handler` holds `lock.lock().await` (762/806), freezing event_monitor and all other PUB/CLAIM. In API mode the event monitor then issues `GET http(s)://{addr}/heartbeat_handler` (219-228) roughly every cycle per entry, and in TCP mode writes HB JSON to the socket (163-172): an unauthenticated peer can make the broker probe/poke any internal host:port indefinitely and amplify (N registrations -> N outbound connections per 200 ms cycle).

**Fix:** Never dial peer-supplied addresses blindly: use the observed source IP of the connection (or require it to match), allow-list ports/CIDRs, add a connect timeout (`tokio::time::timeout`), do the connect outside the State lock, and cap registrations per source.

### S11. CLAIM loop holds State mutex across sleep (1.5 s per unknown-key claim) - trivial broker stall

**Severity:** high  
**Category:** dos  
**Location:** `src/service.rs:806`

`let mut state_loc = lock.lock().await;` is declared inside the `loop` at 806 and stays alive through `sleep(Duration::from_millis(300)).await; continue;` at 839-840, so the State lock is held for 5 x 300 ms per CLAIM with a non-existent key. event_monitor (311, 336, 359, 391) and every PUB (762) block. Contrast: the MSG (886-921) and HB (938-983) loops correctly scope the lock, but hold the connection 50x300 ms = 15 s and 50x702 ms = 35 s respectively, which in TCP mode blocks the serialized accept loop (connection.rs:351).

**Fix:** Drop the guard before sleeping (scope it in a block); replace polling loops with `Notify`/watch channels or return 'not found' immediately and let the client retry; spawn per-connection tasks in `tcp_server`.

### S12. TCP server awaits handler inline: head-of-line blocking and single-panic acceptor death

**Severity:** high  
**Category:** dos  
**Location:** `src/connection.rs:351`

`let _ = handler(Some(shared_stream)).await;` inside the accept loop, and `TcpListener::bind(..).await.unwrap()` at 342. One slow or malicious client (e.g. MSG with unknown id -> 15 s loop, HB -> 35 s loop, or simply not sending anything -> 6 s read timeout) blocks all other connections; a panic anywhere in the handler ends the loop. The loop is spawned once in mode_tcp/operations.rs:40 and never restarted.

**Fix:** `tokio::spawn` per connection with a per-connection timeout and a connection semaphore; log and continue on handler errors; treat bind failure as a startup error returned to main.

### S13. Message framing uses bytes_read < 1024 as end-of-message; breaks on fragmentation, coalescing, exact multiples

**Severity:** high  
**Category:** protocol  
**Location:** `src/connection.rs:225`

`if bytes_read < buf.len() { break; }`. TCP gives no such guarantee: a 3 KB PUB carrying a base64 CA in `root_ca` (operations.rs:236-242) may arrive as 1024 + 900 + 1100 -> parsed after 1924 bytes -> invalid JSON -> panic (finding on :195). A message of exactly 1024·n bytes causes an extra `read` that blocks 6 s then returns TimedOut (220), dropping the message and, in `Heartbeat::monitor`, counting a failure (189-194). Two messages coalesced in one segment parse as invalid JSON. `stream_write` uses `write` not `write_all` (205) and callers ignore the result (238, 271, 821), so partial writes silently truncate.

**Fix:** Adopt explicit framing: 4-byte big-endian length prefix with a hard cap (e.g. 64 KiB) + `read_exact`, or newline-delimited JSON via `BufReader::read_line` with a cap; use `write_all` and check results. Consider replacing the hand-rolled TCP protocol with the HTTP path entirely.

### S14. Unbounded reads and unbounded state growth (memory DoS)

**Severity:** high  
**Category:** dos  
**Location:** `src/connection.rs:216`

`stream_read` appends chunks forever while the peer keeps sending 1024-byte full reads (216-228) with no total cap. HTTP bodies are collected whole with no size limit: connection.rs:283 (`request.collect().await.unwrap()`), api_builder.rs:114, 257, 388, 514, service.rs:1154, 1343. `State.clients` grows by one Payload + one Heartbeat per PUB/CLAIM (service.rs:620-631) with no per-source or global cap and entries are never removed when the monitor task panics (see :236). event_monitor clones the entire State per event (392).

**Fix:** Cap frames (length prefix), wrap bodies in `http_body_util::Limited`, cap registrations per key and per source IP, add TTL-based garbage collection of `clients`, and stop cloning State.

### S15. Heartbeat::monitor unwraps collect_request on party response; non-JSON body evicts entry silently and leaves stale Payload

**Severity:** high  
**Category:** remote-panic  
**Location:** `src/service.rs:236`

`let msg = collect_request(resp.body_mut()).await.unwrap();` runs in the per-heartbeat task spawned by event_monitor (397). The API-mode party endpoints are stubs returning 400 with body "Not implemented" (src/mode_api/operations.rs:249-250, src/operations.rs:543-544), so in two-sided HTTP mode this panics on the first heartbeat. The panic aborts the task before the hb is pushed back (459/472), so the Heartbeat disappears from the deque while its Payload stays in `State.clients` forever and is still handed to claimers by `State::claim`. A malicious service can do this deliberately: PUB under a victim key, answer one HB with garbage, and leave a permanent poisoned entry that is re-claimable every 60 s (691).

**Fix:** Handle non-JSON responses as failures (increment counter), remove from `clients` whenever a Heartbeat is dropped, and implement the party-side /heartbeat_handler in HTTP mode.

### S16. collect_request unwraps body collection; client aborting mid-body panics handler

**Severity:** medium  
**Category:** remote-panic  
**Location:** `src/connection.rs:283`

`request.collect().await.unwrap()` panics if the peer resets the connection mid-body or sends an invalid chunked encoding; `serde_json::to_string(&data).unwrap()` at 291 is safe but `deserialize_message` at 292 panics on any JSON that is not a Message. Called from request_handler (service.rs:737), heartbeat_handler (1233), Heartbeat::monitor (236), publish/claim/collect/send clients (mode_api/operations.rs:205, operations.rs:498, 760, 882).

**Fix:** Return io::Error for body errors and shape mismatches; use `Limited` body; map to 400.

### S17. request_handler panics on ACK/COL/NULL headers and unreachable arm

**Severity:** high  
**Category:** remote-panic  
**Location:** `src/service.rs:745`

`MessageHeader::ACK => panic!(..)`, `COL => panic!(..)` (748), `NULL => panic!(..)` (750) and `_ => panic!("This should not be reached!")` (990). All three headers are valid serde variants, so `{"header":"ACK","body":""}` from any peer panics the broker handler (TCP: acceptor dies; API: connection task dies while holding no lock but after `receive()` already wrote an ACK).

**Fix:** Return `Err`/400 for unexpected headers.

### S18. serde_json::from_str::<MsgBody>().unwrap() on wire bodies in four places

**Severity:** high  
**Category:** remote-panic  
**Location:** `src/service.rs:929`

HB branch of request_handler (929), MSG branch (877), heartbeat_handler HB branch (1438, 1465). Any HB whose body is not a MsgBody (`{"header":"HB","body":"x"}`) panics. From the broker side (`heartbeat_handler` 1438) a rogue broker or anyone connecting to a party's bind_port can trigger it; the TCP party loop at 1017-1027 has no panic recovery so that connection's task dies.

**Fix:** Parse with `?`/match and reply 400; centralize decoding in one typed function.

### S19. Parties' bind_port endpoints are unauthenticated: COL leaks service payload/messages, HB injects messages, MSG triggers relay to broker

**Severity:** high  
**Category:** authz  
**Location:** `src/service.rs:1384`

heartbeat_handler accepts any header from any connection. COL (1384-1435) returns the paired service's full Payload (client side, 1418-1421) or the last delivered message from `GLOBAL_MSGBODY` (service side, 1394-1397) to whoever asks. HB with non-empty `msg` (1462-1465) overwrites `GLOBAL_MSGBODY` directly, bypassing the broker. MSG (1250-1272) makes a client open a new connection to its broker and relay the attacker's body (`msg: message.body.clone()`, `id: 0`), and in HTTP mode loops with `panic!` after 5 failures (1363, 1369). `GLOBAL_MSGBODY` is a process-wide lazy_static (68-70) so in the `api` binary all publish tasks share one inbox.

**Fix:** Only accept HB/MSG on the connection the broker itself opened (or authenticate the broker via mTLS); require the collecting party to authenticate (local unix socket or token); make message storage per-task instead of global.

### S20. https_or_http() everywhere allows plaintext downgrade; broker uses http:// to any party without root_ca

**Severity:** medium  
**Category:** tls  
**Location:** `src/tls.rs:126`

Every HttpsConnector is built with `.https_or_http()` (tls.rs:126, 133; service.rs:1089, 1096, 1281, 1288; operations.rs:462, 470, 712, 720, 837, 845; mode_api/operations.rs:143, 150), so a URI scheme of `http` silently disables TLS even when `--tls` was requested. The broker decides `tls = root_ca.is_some()` (service.rs:548-551) and then builds `http://{addr}/heartbeat_handler` (225) for two-sided HBs and delivers stored messages in clear. Parties post pings/messages to `http://` when `tls` is None (1138-1144, 1327-1333). `Transport::SOCKET` parties are always cleartext.

**Fix:** Use `.https_only()` when TLS is configured; derive TLS from an explicit policy, not from the presence of a CA blob; fail closed.

### S21. No client auth anywhere and trust anchors are supplied by the peer being verified

**Severity:** high  
**Category:** tls  
**Location:** `src/tls.rs:92`

Server: `ServerConfig::builder().with_no_client_auth()` (92). Clients: `with_no_client_auth()` at tls.rs:123, 158; operations.rs:445, 701, 826. Worse, the CA the broker uses to verify a party is the base64 DER shipped by that same party inside `Payload.root_ca` (operations.rs:230-245, 301-316 encode; service.rs:543 -> tls.rs:107-123 decode into the RootCertStore). An attacker publishes with its own CA and self-signed leaf and passes verification. TLS therefore provides confidentiality against passive observers only; it authenticates nobody. `ROOT_PATH` (tls.rs:149-154, operations.rs:438-442, 697, 822) exists but is not used for peer verification on the broker side.

**Fix:** Configure trust anchors from operator config only (ROOT_PATH), remove `root_ca` from the wire Payload, enable `with_client_cert_verifier` (mTLS) and bind PUB/CLAIM authorization to the client certificate identity.

### S22. Native root store fallback: any public CA can impersonate broker/parties; panics in minimal containers

**Severity:** low  
**Category:** tls  
**Location:** `src/tls.rs:65`

`load_native_certs().unwrap()` (65) and `with_native_roots().unwrap()` (132; service.rs:1095, 1287; operations.rs:469, 719, 844) fall back to the OS trust store whenever no CA is provided. For an internal mesh this means a certificate from any public CA for the target IP/name is accepted. `CertificateResult::unwrap` panics when the platform store has errors (vendor/rustls-native-certs/src/lib.rs:147-155); the Dockerfile's `alpine:3.18` final stage installs no `ca-certificates` (Dockerfile:49-65), so TLS clients in the image panic at first use.

**Fix:** Require an explicit trust anchor set for mesh traffic; never fall back to native roots implicitly; install ca-certificates only if public CAs are intentionally trusted.

### S23. ALPN list contains invalid 'http/1.0'; server advertises h2 while every client is http1-only

**Severity:** low  
**Category:** tls  
**Location:** `src/tls.rs:96`

`server_config.alpn_protocols = vec![b"h2", b"http/1.1", b"http/1.0"]`. `http/1.0` is not a registered ALPN protocol id; clients use `.enable_http1()` only. Harmless functionally but signals untested TLS config.

**Fix:** Advertise only `http/1.1` (or `h2` + `http/1.1` if h2 is enabled on clients).

### S24. ServerName computed and discarded (dead TLS code)

**Severity:** low  
**Category:** tls  
**Location:** `src/operations.rs:446`

`let _server_name = ServerName::try_from(parsed_url.host_str().unwrap()).map_err(..).unwrap();` is never used. hyper-rustls derives SNI/verification name from the request URI, so verification still happens, but `url::Url::parse(&inputs.host.to_string()).unwrap()` at 435 panics for `Transport::SOCKET` hosts (e.g. REST `host:"10.0.0.1:8000"`) and `host_str().unwrap()` panics on hosts without authority.

**Fix:** Delete the dead code; validate Addr transport before entering the HTTP path and return an error.

### S25. Full requests, payloads and broker state logged/printed (information disclosure)

**Severity:** low  
**Category:** logging  
**Location:** `src/api_builder.rs:20`

`info!("Entering handle_list_interfaces with request: {:?}", request)` and the same for every REST handler (55, 111, 254, 385, 511) log all headers. `println!` of received payloads including `root_ca` and all interface IPs: operations.rs:379, 503; mode_api/operations.rs:200 prints the raw HTTP result. The broker prints its entire client table to stdout on every PUB/CLAIM/remove (service.rs:646, 707 via 771, 791, 874) - keys, IPs, ports of every tenant. `trace!` dumps the rustls ClientConfig (tls.rs:162) and every deque state (563, 889, 941). Log level defaults to warn (tcp.rs:46) but info/println are unconditional.

**Fix:** Remove println! from library code; log request method/path/remote addr only; never log Payload.root_ca or full state; add a redaction Debug impl for Payload.

### S26. .env with another developer's certificate and key paths is committed

**Severity:** medium  
**Category:** secrets  
**Location:** `.env:1`

`.env` tracked in git contains `CERT_PATH=/Users/sofiamorris/...` and `KEY_PATH=/Users/sofiamorris/...`. Not the key material itself, but it leaks a personal home path, hard-codes a developer machine, and normalises committing .env files. `.gitignore` does not exclude `.env` (only `.dockerignore` does).

**Fix:** `git rm --cached .env`, add `.env` to .gitignore, provide `.env.example`; consider history rewrite if the repo is published.

### S27. .gitignore typos mean server.key and role-binding files are NOT ignored

**Severity:** medium  
**Category:** secrets  
**Location:** `.gitignore:7`

Lines 7 and 9 read `view-events-rolebinding.yam` and `server.keyl` (typos). `server.key` (the private key the TLS setup expects) and `server.crt` are therefore not ignored and a `git add -A` would commit the private key. `view-events-rolebinding.yaml` is in fact tracked despite the intent to ignore it.

**Fix:** Fix to `server.key`, `*.key`, `*.pem`, `*.crt`, `*.csr`; add a pre-commit secret scanner.

### S28. Dockerfile CMD is broken and requires TLS env that is never provided

**Severity:** medium  
**Category:** deployment  
**Location:** `Dockerfile:73`

`CMD ["-n","en0","--ip-version","4","--operation","listen","--bind-port","12000","--tls"]` - the CLI has no `--operation` flag (operation is positional, cli.rs:17-31), `en0` is a macOS interface name (Linux containers have eth0), so `only_or_error` panics (operations.rs:182). `--tls` needs CERT_PATH/KEY_PATH (tls.rs:22, 34 `expect`) which compose.yaml does not set (compose.yaml:11-16). Base image `alpine:3.18` (line 49) is end-of-life since 2025-05; RUST_VERSION 1.83 (line 9) vs toolchain 1.98. No `ca-certificates`. Non-root user is correctly used (53-62).

**Fix:** Fix CMD to the real CLI, pass certs via secrets/volumes, bump base images, add HEALTHCHECK, install ca-certificates only if needed.

### S29. 18 MB Docker image tarball and 156 MB stale rustdoc tracked in git

**Severity:** low  
**Category:** repo-hygiene  
**Location:** `nsm-dev-buildx-latest.tar:1`

`nsm-dev-buildx-latest.tar` (18.5 MB, OCI image with Entrypoint /bin/server, Env PATH only - no secrets found in config) and `docs/` (10,971 files of 2024 rustdoc for a `nsm` binary that no longer exists) are tracked, as is `src/.DS_Store`. Binary blobs in git cannot be code-reviewed and inflate clones; a future rebuild could embed secrets.

**Fix:** Remove from git (and history if size matters), publish images to a registry, generate docs in CI.

### S30. Kubernetes RoleBinding grants role to the namespace default ServiceAccount

**Severity:** low  
**Category:** deployment  
**Location:** `view-events-rolebinding.yaml:7`

Binds `view-events` to `ServiceAccount default` in `nsm-dev`. Binding to `default` gives every pod in the namespace the permission. The referenced Role and other RBAC files are git-ignored (.gitignore:2-6) so the actual grants cannot be reviewed.

**Fix:** Create a dedicated ServiceAccount for the broker pod with `automountServiceAccountToken: false` unless needed; commit the Role for review.

### S31. Integer truncation and sign wrap in REST parameter parsing

**Severity:** low  
**Category:** integer  
**Location:** `src/api_builder.rs:172`

`bind_port`/`service_port`: `as_i64() ... as i32` (172-173, 183-184, 315-316) truncates (4294967376 -> 80). `key`: `as_i64() as u64` (194-195, 326-327, 457-458, 584-585) turns -1 into u64::MAX while keys above i64::MAX (valid for the CLI `value_parser!(u64)`, cli.rs:100) are rejected with 400 - inconsistent key space between binaries. `Addr.port` is i32 (connection.rs:41) and negative/oversized values flow into `format!("{}:{}")` for `connect`/`bind` (connection.rs:200, 341) where bind `.unwrap()`s (342) and `SocketAddr` parse `.unwrap()`s (operations.rs:553).

**Fix:** Parse ports as u16 and keys as u64 via `as_u64()`; make `Addr.port: u16`; reject out-of-range with 400.

### S32. u64 underflow in State::claim with wire-controlled service_claim; overflow-checks off in release

**Severity:** medium  
**Category:** integer  
**Location:** `src/service.rs:691`

`if current_ecpoch - v.service_claim > self.timeout` where `service_claim` comes verbatim from the peer's Payload (PUB sets 0 legitimately, CLAIM sets `epoch()` at operations.rs:324, but the broker never overwrites it before storing, 629-631). A peer sending `service_claim: 18446744073709551615` makes the subtraction underflow: panic in debug builds, wrapping in release (Cargo.toml has no `overflow-checks`), producing a huge value so the entry is always claimable - defeating the single-claim/timeout semantics. `Instant::now() - self.fail_counter.last_increment` (279) is fine.

**Fix:** Ignore wire `service_claim`/`id`/`service_id` (server-assigned only); use `saturating_sub`/`checked_sub`; add `overflow-checks = true` in release or debug-assert invariants.

### S33. State::claim can hand out a client Payload as a service (address leak, wrong pairing)

**Severity:** medium  
**Category:** authz  
**Location:** `src/service.rs:689`

`State::add` pushes both PUB and CLAIM payloads into the same `clients[key]` Vec (628-631). `claim()` iterates every entry and returns the first with `now - service_claim > timeout` (689-694) without checking `item.service_id == item.id` (the service marker used at 657). 60 s after a client claims, its own Payload (`service_port: -1`, `service_addr` = client IP) becomes claimable and is returned to the next claimer, leaking the client's address and breaking pairing.

**Fix:** Keep services and clients in separate maps; filter on the service marker; unit-test claim semantics.

### S34. event_monitor shared fail tuple is racy and evicts on the first non-OK heartbeat

**Severity:** medium  
**Category:** dos  
**Location:** `src/service.rs:335`

All spawned monitor tasks write one shared `(fail_count,key,id,service_id,fail_id)` tuple (305, 426-432) that the main loop reads once per 200 ms (332-356); results overwrite each other so removals are lost or misattributed. `fail_id = hb.service_id` for any status != OK (408-411), so `shared_data.4 != 0` triggers `rmv` immediately (335-337) and the hb is dropped (433, 477) after a single 3 s/6 s timeout - the FailCounter/10 logic (335, 433) is effectively dead. A transient network blip evicts a service; combined with the CLAIM-lock stall (806) an attacker can make heartbeats time out and mass-evict.

**Fix:** Carry results per heartbeat (return value / channel), remove only when fail_count reaches threshold, and add tests for the event loop.

### S35. Anonymous GET /heartbeat_handler resets liveness watchdog; prefix routing

**Severity:** low  
**Category:** authz  
**Location:** `src/connection.rs:314`

Any `GET` whose path starts with `/heartbeat_handler` sets `GLOBAL_LAST_HEARTBEAT = Some(Instant::now())` before the handler runs, so anyone can keep a party alive indefinitely (or, in the api binary, keep the process from exiting). Routes use `starts_with` (311, 314; api.rs:140-157).

**Fix:** Update liveness only after a successfully authenticated/validated HB from the broker; exact-match paths.

### S36. rustls crypto-provider feature plumbing is inert; provider only present via hyper-rustls defaults

**Severity:** low  
**Category:** dependencies  
**Location:** `Cargo.toml:33`

`[features] ring = [] / aws-lc-rs = []` are empty, so `#[cfg(feature = "ring")] rustls::crypto::ring::default_provider().install_default()` (tls.rs:81-84, operations.rs:651-654, 785-788) never compiles in, and `rustls = { default-features = false }` enables no provider. The build works only because hyper-rustls 0.27 default features pull `rustls/aws_lc_rs` (vendor/hyper-rustls/Cargo.toml:150-157). Disabling hyper-rustls defaults or bumping would yield a runtime panic 'no process-level CryptoProvider'. `rustls-platform-verifier` is declared optional and never enabled; `threadpool` is unused.

**Fix:** Pick one provider explicitly (`rustls = { features = ["aws-lc-rs"] }` or ring), install it once in main, drop the fake features, remove unused deps.

### S37. HTTP servers have no connection limits, timeouts or TLS handshake timeout

**Severity:** medium  
**Category:** dos  
**Location:** `src/mode_api/operations.rs:92`

Accept loops (mode_api/operations.rs:92-129, 308-344; operations.rs:602-638; api.rs:117-127) spawn a task per connection with `incoming.accept().await.unwrap()` (93, 309, 603 - panic on accept error such as EMFILE kills the listener) and `tls_acc.accept(tcp_stream).await` with no timeout, no max-connections, no header/body timeouts beyond hyper defaults. Slowloris or fd exhaustion is straightforward.

**Fix:** Wrap accept/handshake in timeouts, use a Semaphore for max in-flight connections, handle accept errors by logging + backoff, set hyper `header_read_timeout`/`timer`.

### S38. REST publish/claim root_ca is a server-side file path read and exfiltrated to attacker host

**Severity:** medium  
**Category:** file-read  
**Location:** `src/operations.rs:232`

`publish` (230-245) and `claim` (301-316) do `fs::File::open(inputs.root_ca)` then `rustls_pemfile::certs(..)` and base64-encode every CERTIFICATE block into `Payload.root_ca`, which is POSTed to `inputs.host`. Both `root_ca` and `host` come straight from the unauthenticated REST body (api_builder.rs:210-213, 342-345). An attacker can read any PEM-formatted file the api process can access (e.g. the operator's CERT_PATH, /etc/ssl/certs/*) and receive it at their own listener; non-existent paths return distinguishable errors (existence oracle via timing/logs).

**Fix:** Never take file paths from the network; configure CA on the server side only; if a CA must be specified per request, accept the PEM content with a size cap, not a path.

### S39. Unhandled panics/asserts in CLI and unawaited sleeps hide protocol bugs

**Severity:** low  
**Category:** robustness  
**Location:** `src/cli.rs:209`

`assert!(args.contains_id(..))` (209, 230-232, 261-264, 295, 320-321) and `.unwrap()` on parsed host (234-236, 266-268, 297-299, 323-325) panic instead of printing usage; `_ => panic!()` (347). `let _ = sleep(..)` without `.await` at operations.rs:370, service.rs:532, 1184 are no-ops, so retry loops spin immediately.

**Fix:** Use clap subcommands with required args per subcommand; `.await` the sleeps or delete them.

### S40. Party-side HTTP heartbeat endpoint is a stub returning 400, so HTTP two-sided mode cannot work and triggers broker-side panics

**Severity:** medium  
**Category:** protocol  
**Location:** `src/mode_api/operations.rs:238`

The handler installed on a published service's bind_port (238-255) and on a client's (operations.rs:530-549) ignores the request and returns 400 'Not implemented'. In two-sided mode (`ping=false`, the default) the broker's `Heartbeat::monitor` receives this and panics at service.rs:236, and the party's watchdog (`GLOBAL_LAST_HEARTBEAT` set only on GET /heartbeat_handler, which the broker does call before the 400) keeps it alive while the broker has already lost it. MSG/COL delivery in HTTP mode is therefore impossible; only `--ping` mode functions.

**Fix:** Wire `heartbeat_handler_helper` back into the HTTP handler (as the commented code intended) after fixing its panics, or drop two-sided HTTP mode from the supported matrix and reject `ping=false` on the HTTP path.


# Audit: Wire protocol and state machine

Findings reference `main` at commit `edd23a33` (2026-09-24), the state before the cleanup branches. Line numbers will drift as the cleanup lands; the file names are stable enough to locate each item.

Severity counts: 5 critical, 11 high, 10 medium, 5 low, 1 info.

## Summary

Wire-protocol/state-machine trace of nsm (bins "tcp" and "api"), verified from code. Headline: (1) the "10 failures to drop" rule is NOT what runs: any single non-OK/non-ACCEPTED monitor status sets fail_id != 0 and the entity is removed on the next 200 ms tick (service.rs:408-411,433,335). (2) `send` is broken end-to-end on both transports since commit 3be4b0bc: the relay uses MsgBody.id = 0 (service.rs:1265,1298) while the broker looks up `e.id == msg_body.id` (service.rs:891); no Heartbeat has id 0 (seq starts at 1). (3) On the HTTP path the peer-side handler for /heartbeat_handler and /request_handler is a 400 "Not implemented" stub (mode_api/operations.rs:238-255, operations.rs:530-549), so HTTP two-sided heartbeats, HTTP send and HTTP collect cannot work; the broker monitor task panics on the non-JSON 400 body (service.rs:236) leaving a zombie claimable Payload in State. (4) TCP broker accept loop awaits request_handler inline and unwraps every parse (connection.rs:351,194-195,222; service.rs:736): a bare connect+close or non-JSON kills the listener task while the event loop keeps running. (5) State::add reconnect scan can spin forever holding deque+State locks (service.rs:559-596). (6) Re-claim mutates a State clone, never updates hb.service_id, never notifies the client, repeats every cycle (service.rs:392,438-467). (7) State::claim hands out client payloads (service_port -1) after 60 s and re-issues live services after 60 s (service.rs:685-701). (8) Shared (fail_count,key,id,service_id,fail_id) tuple is last-writer-wins across concurrent monitor tasks (service.rs:305,332,425-432). (9) TCP framing has no delimiter; TCP CLAIM reply is two back-to-back JSON objects that can coalesce (connection.rs:212-231,271-276; service.rs:821). (10) Connect-back retry sleeps are never awaited and add() errors are discarded (service.rs:532,766,822; operations.rs:370). The `extra` field holds the as-built protocol spec, invariants to preserve, and the TCP-vs-HTTP divergence table.

## Detail

## As-built protocol (verified from code)

### Transport & framing
- **TCP** (`tcp` bin): raw TCP, one JSON `Message` per write, no delimiter. Reader loops on 1024-byte chunks until a chunk <1024 (connection.rs:212-231), 6 s idle timeout per read. `connection::send` = write then (unless header ACK) one read. `connection::receive` = read, then auto-write `{"header":"ACK","body":""}` for anything other than HB/ACK (271-276).
- **HTTP(S)** (`api` bin): same `Message` JSON as request/response body. Peer/broker router `api_server` (connection.rs:299-327): `POST /request_handler*` -> handler; `GET /heartbeat_handler*` -> sets GLOBAL_LAST_HEARTBEAT=now then handler; else 404. Bodies are read whole with `collect_request` (unwrap on non-JSON). TLS: server side needs env CERT_PATH/KEY_PATH (tls.rs:22,34), client root store from ROOT_PATH or native roots. TLS exists only on the HTTP path.
- REST front-end (api.rs:131-168, port 0.0.0.0:8080): GET /list_interfaces, GET /list_ips, POST /publish, GET /claim, GET /collect, POST /send, JSON bodies with string-typed booleans.

### Messages
`Message{header: HB|ACK|PUB|CLAIM|COL|MSG|NULL, body: String}` (connection.rs:147-186).
`Payload{service_addr: [String], service_port: i32, service_claim: u64, interface_addr: [String], bind_port: i32, key: u64, id: u64, service_id: u64, root_ca: Option<String /*base64 DER lines*/>, ping: bool}` (service.rs:34-54). `MsgBody{msg, id, service_id}` (58-65).

### Broker state
`State{clients: HashMap<key, Vec<Payload>>, timeout: 60, seq: 1.., deque: VecDeque<Heartbeat>, running}` (486-509). Services and clients share the Vec. `Heartbeat{key,id,service_id,addr "ip:bind_port",stream|client,tls,fail_counter,msg_body,ping}`.
IDs: `id = seq` at add, `seq += 1` (609,629,632). Service: `service_id = id`. Client: `service_id = claimed service's id` (603-605,810). Test "is service" = `id == service_id` (657). PUB: `service_claim = 0`; CLAIM payload: `service_claim = epoch()` (operations.rs:324).
`State::claim(key)`: first payload under key with `epoch - service_claim > 60` -> set `service_claim = epoch`, return it (685-701). So a claim is a 60 s lease; a service becomes claimable again after 60 s; unclaimed = `service_claim 0`; client removal resets the service's service_claim to 0 (668).

### Flows
**PUB/TCP**: peer `send(PUB{Payload})` -> broker `receive` auto-ACK (empty body) -> `add(payload,0,TCP)`: broker connects to `service_addr[0]:bind_port` (6 immediate attempts, sleep never awaited) -> push Heartbeat -> peer starts `tcp_server(bind_port)` after ACK; each accepted connection gets an infinite `heartbeat_handler` loop.
**PUB/HTTP**: `POST {broker}/request_handler` PUB (1 s pre-sleep, 6 s timeout, 6 attempts) -> reply `ACK{body: seq}` -> peer rewrites payload.id=service_id=seq -> broker `add(..,API)` builds hyper client from payload.root_ca (tls = root_ca.is_some()) -> peer serves bind_port with a stub 400 handler; if !ping: watchdog exits after 5 s warmup when last HB >10 s old (only once one HB has arrived); if ping: `ping_heartbeat`.
**CLAIM/TCP**: `send(CLAIM{Payload})` -> auto-ACK(empty) -> broker `claim(key)` retried 6x/300 ms -> `ACK{body: service Payload JSON}` written on same stream (821) -> `add(payload, service.service_id, TCP)` connect-back -> client reads until an ACK arrives (operations.rs:369-391), stores service payload, starts tcp_server(bind_port). Failure: NULL not sent (write not awaited), stream closed.
**CLAIM/HTTP**: POST CLAIM -> `200 ACK{service Payload}` or `400 NULL` -> `add(..,API)`; client serves stub handler; ping or watchdog as above.
**HB two-sided (ping=false)**: broker tick (200 ms) pops one hb, spawns `monitor`: TCP write `HB{MsgBody}` then read (6 s); HTTP `GET http(s)://ip:bind_port/heartbeat_handler` body HB{MsgBody}, 3 s timeout. Peer `heartbeat_handler`: if msg empty -> reply `HB{body: stored service payload | ""}`; else store into GLOBAL_MSGBODY and echo `HB{same body}`. Broker: non-empty reply => fail_count=0, OK; else fail. Any non-OK => fail_id = service_id => not re-queued => rmv next tick (single failure removes; FailCounter 10x5 s is unreachable). Re-queue happens only after monitor completes, so period ~ N x 200 ms.
**HB one-sided (ping=true, HTTP only)**: peer POSTs `HB{MsgBody{"",id,service_id}}` to `{broker}/request_handler` every 1 s (10 s timeout, exits process after 10 failures). Broker finds hb with matching (service_id,id) (50 x 702 ms) and sets `last_increment = now`, replies `HB{same}`. Monitor for ping entries: `now - last_increment > 60 s` => GONE => removed; else ACCEPTED. Client pings carry the SERVICE's ids.
**SEND**: `send` -> client bind_port: TCP `send(MSG{"\"text\""})` (client writes nothing back; sender times out 6 s); HTTP `POST /request_handler` (stub 400 today). Client relays `MSG{MsgBody{msg, id:0, service_id}}` to broker (new TCP conn or POST). Broker looks up `e.id == 0` (never matches, 50 x 300 ms) -> intended: set `hb.msg_body`, next HB to service carries it -> service stores GLOBAL_MSGBODY (never cleared; re-sent every HB).
**COLLECT**: `collect` -> peer bind_port with `COL{""}`: TCP reply `COL{body}`, HTTP `GET /heartbeat_handler` reply `ACK{body}`. Service replies `to_string(GLOBAL_MSGBODY.msg)`; client replies its stored service Payload JSON.
**Failure/removal**: main loop `if fail_count==10 || fail_id!=0 -> rmv(key,id,service_id)`; rmv removes payload by id, deletes key when empty, returns PUB{service_id} for services (then `service_id` var arms re-claim for clients with that service_id) or CLAIM after resetting the service's service_claim=0. Re-claim: task calls `claim()` on a State CLONE, rebuilds hb with same service_id, re-queues; on Err sets fail_count=10 -> client removed next tick. Client never notified.
**Reconnect (add)**: same key + same (service_addr, service_port) => find hb by old id in deque (unbounded loop) and, if `first_increment` <60 s ago, swap stream/client/tls/ping in place and reuse old id (no new payload).

### Constants
200 ms tick; 6 s TCP read timeout (everywhere); 3 s HTTP HB request timeout; 6 s HTTP request timeout for register/send/collect; 10 s peer watchdog (HTTP, 5 s warmup, 500 ms poll); 10 s ping request timeout, 1 s ping interval, exit after 10 failures; 60 s ping staleness; 60 s claim lease; FailCounter interval 5 s, threshold 10 (dead); claim retry 6 x 300 ms; MSG lookup 50 x 300 ms; HB lookup 50 x 702 ms; add reconnect scan "10" (element-count based); connect-back 6 attempts (0 ms apart); HTTP register/claim 6 attempts, 1 s apart.

## Semantics the rewrite MUST preserve
1. Key-based rendezvous: N services and clients share a `key`; a claim returns one service Payload (service_addr[0], service_port) to the client.
2. Broker is the only fixed address; broker initiates heartbeats to each party's bind_port (two-sided) or accepts pings (one-sided, per-Payload `ping` flag).
3. Identity: monotonically increasing `id`; service `service_id == id`; client `service_id` = its service's id; `key` groups them. Peers learn their own id from the registration reply (HTTP) and the client learns the service's Payload from the CLAIM ACK.
4. Claim lease/exclusivity via `service_claim` (currently 60 s), released when the client is removed.
5. Liveness -> removal; a service's removal triggers re-claim of its clients to another service under the same key; a client removal frees the service.
6. Reconnect within a grace window keeps the same id and replaces the transport handle.
7. `send` semantics: message addressed to a client's bind_port is relayed via broker into the paired service's next heartbeat; `collect` on the service returns the last message; `collect` on the client returns the service Payload.
8. Operation surface: list_interfaces, list_ips, listen, publish, claim, collect, send; Addr grammar `host:port | http://host:port | https://host:port`.
9. Root CA material travels in the Payload (base64 DER lines) so the broker can verify the peer's HTTPS heartbeat endpoint.

## TCP vs HTTP behavioral differences (today)
- TLS: HTTP only; TCP ignores --tls/root_ca (still ships root_ca).
- PUB reply: TCP empty auto-ACK (peer keeps id 0); HTTP ACK{seq} and peer rewrites id/service_id.
- CLAIM reply: TCP two messages on one stream (auto-ACK then payload ACK) or silent close on failure; HTTP one 200 ACK or 400 NULL.
- Registration: TCP broker connects back synchronously inside add (race, error dropped); HTTP just builds a client.
- HB: TCP persistent stream, 6 s read timeout, peer replies; HTTP GET per beat, 3 s timeout, peer currently 400 stub -> broker task panic.
- Ping: HTTP only; TCP --ping produces an entry that is GONE after 60 s.
- Peer liveness: TCP exit(0) on any 6 s read timeout on any connection; HTTP exit when >10 s since last HB but only after the first HB.
- Broker request concurrency: TCP serial accept loop; HTTP one hyper task per connection (but shared State lock).
- Broker MSG/HB acknowledgements: HTTP echoes header; TCP writes nothing (send_msg TCP always times out).
- COL reply header: TCP COL, HTTP ACK; collect TCP ignores header, HTTP requires ACK.
- MSG relay: TCP opens a new connection to broker and uses `connection::send`; HTTP POSTs with 1 s pre-sleep, panics after 5 failures.
- Scheme selection: registration via Addr scheme; pings/send/collect via --tls flag; broker HB via root_ca presence.
- Broker address parsing: TCP accepts `ip:port`; API panics on it.

## Findings

Ids are `P` plus the finding number, in the order the reviewer reported them (not by severity).

| # | Severity | Category | Finding | Location |
|---|---|---|---|---|
| P1 | critical | protocol-defect | MSG relay uses id=0; broker lookup never matches, send never delivered | `src/service.rs:1265` |
| P2 | critical | state-machine | Single failed heartbeat removes entity; 10-failure threshold unreachable | `src/service.rs:408` |
| P3 | critical | protocol-defect | HTTP peer heartbeat/request handler stubbed 400; broker monitor panics on non-JSON body | `src/mode_api/operations.rs:238` |
| P4 | critical | robustness | TCP broker accept loop dies on first malformed or empty connection | `src/connection.rs:351` |
| P5 | critical | deadlock | State::add reconnect scan can loop forever while holding deque and State locks | `src/service.rs:559` |
| P6 | high | state-machine | Re-claim of orphaned client operates on a State clone and never updates the client | `src/service.rs:438` |
| P7 | high | race | Shared monitor-result tuple is last-writer-wins; removals are lost | `src/service.rs:305` |
| P8 | high | state-machine | State::claim treats client payloads as claimable and re-issues live services after 60 s | `src/service.rs:685` |
| P9 | high | race | TCP connect-back race: retry sleeps not awaited, add() errors discarded | `src/service.rs:532` |
| P10 | high | wire-format | No message framing on raw TCP; back-to-back writes coalesce, 1024-multiple messages stall | `src/connection.rs:212` |
| P11 | high | protocol-defect | HTTP ping-mode client pings with the SERVICE's ids, not its own | `src/operations.rs:593` |
| P12 | high | architecture | Process-global heartbeat/msg state and process::exit make the REST server single-tenant and self-terminating | `src/operations.rs:54` |
| P13 | high | scalability | Heartbeat period grows linearly with registered entities; peers self-terminate at ~30-50 entries | `src/service.rs:480` |
| P14 | high | protocol-defect | TCP CLAIM failure: NULL reply not awaited, stream dropped, client panics on EOF | `src/service.rs:851` |
| P15 | high | api-semantics | REST front-end returns before operations run and /collect never returns collected data | `src/api_builder.rs:482` |
| P16 | high | liveness | Broker accept loop is serialized behind long retry loops in request_handler | `src/service.rs:885` |
| P17 | medium | transport-divergence | --ping on the TCP binary registers a ping-mode entry nobody pings; removed after 60 s | `src/operations.rs:258` |
| P18 | medium | transport-divergence | Heartbeat.tls derived from root_ca presence, not from the peer's TLS mode | `src/service.rs:548` |
| P19 | medium | dead-code | event_monitor service_claim reset block is dead (fail_id already zeroed) | `src/service.rs:357` |
| P20 | medium | robustness | TCP peer exits process on any read error on any accepted connection | `src/service.rs:1215` |
| P21 | medium | state-machine | HTTP peer watchdog never fires if no heartbeat was ever received | `src/operations.rs:568` |
| P22 | medium | transport-divergence | PUB ACK body differs: HTTP returns pre-add seq (wrong on reconnect), TCP returns empty | `src/service.rs:777` |
| P23 | medium | transport-divergence | Reply header asymmetry between transports (COL vs ACK, MSG echo vs none) | `src/service.rs:1394` |
| P24 | low | wire-format | User message is JSON-encoded three times along send->collect | `src/operations.rs:792` |
| P25 | low | state-machine | Delivered message never cleared; re-sent in every HB | `src/service.rs:149` |
| P26 | low | protocol-defect | Two-sided monitor never validates the HB reply | `src/service.rs:262` |
| P27 | medium | state-machine | Reconnect matching by (service_addr, service_port) collides for all clients on one host | `src/service.rs:556` |
| P28 | medium | protocol-defect | Broker replies empty 200 when MSG/HB target not found; peers unwrap and panic | `src/service.rs:924` |
| P29 | low | input-handling | API mode panics on socket-style broker address | `src/operations.rs:435` |
| P30 | low | wire-format | GET /heartbeat_handler requests carry JSON bodies | `src/service.rs:217` |
| P31 | medium | robustness | Peer-controlled root_ca decoded with unwrap inside broker add() | `src/tls.rs:111` |
| P32 | info | design | In-flight Heartbeats are invisible to MSG/HB/add lookups; all rely on polling retries | `src/service.rs:373` |

### P1. MSG relay uses id=0; broker lookup never matches, send never delivered

**Severity:** critical  
**Category:** protocol-defect  
**Location:** `src/service.rs:1265`

heartbeat_handler relays a received MSG to the broker as MsgBody{msg, id: 0, service_id} (TCP at 1263-1267, HTTP at 1296-1300). request_handler's MSG branch searches the deque with `if e.id == msg_body.id` (891). Heartbeat.id is assigned from State.seq which starts at 1 (505,609), so id 0 never matches; the loop retries 50x300ms then logs "'MSG' giving up" (924-926). Commit 3be4b0bc changed `id: service_id` to `id: 0` without changing the lookup. Result: Heartbeat.msg_body is never set, GLOBAL_MSGBODY on the service is never populated, `collect` from the service always returns "".

**Fix:** Look up by service id: set MsgBody.id = service_id in the relay (or match on `e.id == msg_body.service_id`/`e.service_id`). Add an integration test send->collect.

### P2. Single failed heartbeat removes entity; 10-failure threshold unreachable

**Severity:** critical  
**Category:** state-machine  
**Location:** `src/service.rs:408`

In the spawned monitor task, `else if status != StatusCode::OK { hb.service_id as i64 }` sets fail_id for ANY non-OK status. Every failure branch in Heartbeat::monitor returns BAD_REQUEST or REQUEST_TIMEOUT (180,186,192,205,248,252,269), and hb.service_id is always >=1, so fail_id != 0 on the first failure. The task then skips re-queueing (433-435, "Dropping event") and the main loop calls rmv on the next tick because `shared_data.4 != 0` (335). FailCounter (10 x 5 s) is effectively dead code; a single 6 s TCP read timeout or one connection reset drops a service or client.

**Fix:** Decide the intended semantics (fail_id only for GONE pings; two-sided uses fail_count>=10) and encode it explicitly; unit-test that a transient failure re-queues the entry.

### P3. HTTP peer heartbeat/request handler stubbed 400; broker monitor panics on non-JSON body

**Severity:** critical  
**Category:** protocol-defect  
**Location:** `src/mode_api/operations.rs:238`

HTTP publish (mode_api/operations.rs:238-255) and HTTP claim (operations.rs:530-549) install a handler returning 400 "Not implemented" for every request. Broker Heartbeat::monitor treats Ok(resp) of any status as success and calls `collect_request(resp.body_mut()).await.unwrap()` (service.rs:236) on the body "Not implemented" -> serde error -> panic inside the tokio task. The task never writes the shared tuple and never re-queues the hb, so the entity vanishes from the deque but its Payload remains in State.clients and stays claimable forever. Consequently HTTP two-sided HB, HTTP send (POST /request_handler on the peer) and HTTP collect (GET /heartbeat_handler on the peer) are all non-functional; only registration + ping mode works in the api binary.

**Fix:** Re-wire heartbeat_handler_helper into the peer's api_server (as the commented code intended); make monitor check resp.status() and treat non-2xx / non-JSON as a failure rather than unwrap.

### P4. TCP broker accept loop dies on first malformed or empty connection

**Severity:** critical  
**Category:** robustness  
**Location:** `src/connection.rs:351`

tcp_server awaits `handler(Some(shared_stream)).await` inline for every accepted connection. request_handler does `receive(&s).await.unwrap()` (service.rs:736); stream_read returns Ok("") on EOF (connection.rs:225-230) and deserialize_message does `serde_json::from_str(payload).unwrap()` (195); from_utf8(...).unwrap() at 222. A plain connect+close (port scan, health check) or any non-JSON bytes panics the tcp_server task spawned in mode_tcp/operations.rs:40-42; listen() only awaits event_loop (53), so the process keeps running with a dead listener. Also panics on ACK/COL/NULL headers (service.rs:745-750) and on only_or_error(&p.service_addr) when a peer sends !=1 address (514).

**Fix:** Spawn a task per connection, return Result on parse errors, never unwrap on peer input.

### P5. State::add reconnect scan can loop forever while holding deque and State locks

**Severity:** critical  
**Category:** deadlock  
**Location:** `src/service.rs:559`

`while counter < 10 { let mut deque_loc = self.deque.lock().await; ... find_map(|e| if e.id == item.id {Some(e)} else {counter+=1; None}) }`. counter only increments for NON-matching deque elements. If the deque is empty (the only entry is currently popped for monitoring) or the matching hb is found but `first_increment` is >= 60 s old (574), the loop never reaches 10 and re-acquires the deque lock forever; request_handler holds the State lock for the duration (762/806), so the event loop (311,336,359,382,391) and every other request block: full broker deadlock. Trigger: re-publish from the same host/port within a heartbeat cycle.

**Fix:** Replace with a single bounded search plus timed retries that release locks between attempts; make reconnect an explicit state transition.

### P6. Re-claim of orphaned client operates on a State clone and never updates the client

**Severity:** high  
**Category:** state-machine  
**Location:** `src/service.rs:438`

event_monitor clones the whole State each tick (`let mut state_clone = state_loc.clone()` 392) and the task calls `state_clone.claim(hb.key)` (440): the service_claim update is lost. The rebuilt Heartbeat keeps `service_id: hb.service_id` (447) i.e. the DEAD service's id, `service_id` (306) is never reset to -1 after a PUB removal (343), so `hb.service_id == service_id` matches again on every cycle and claim() is called each tick. The client is never told about the new service (no message is sent); its stored service_payload still points at the dead service, and its Payload.service_id in State is unchanged, so `rmv` bookkeeping later mismatches.

**Fix:** Perform re-claim under the real State lock, update Payload.service_id and hb.service_id, and push the new service payload to the client in the next HB (or drop the feature explicitly).

### P7. Shared monitor-result tuple is last-writer-wins; removals are lost

**Severity:** high  
**Category:** race  
**Location:** `src/service.rs:305`

`data: Arc<Mutex<(fail_count,key,id,service_id,fail_id)>>` is written by every concurrently spawned monitor task (425-432) and read once per tick (332). The main loop holds the tuple lock across its 200 ms sleep (332..480), so several tasks queue and overwrite each other; only the last write is examined. A failing entity whose task wrote (.., fail_id!=0) can be overwritten by a healthy one -> the failed hb was not re-queued (475-478) but rmv is never called -> Payload stays in State.clients as a claimable zombie.

**Fix:** Use a channel (mpsc) of MonitorOutcome events processed one by one, or have the task call rmv itself under the State lock.

### P8. State::claim treats client payloads as claimable and re-issues live services after 60 s

**Severity:** high  
**Category:** state-machine  
**Location:** `src/service.rs:685`

claim() iterates ALL payloads under the key (services and clients live in the same Vec) and returns the first with `epoch - service_claim > timeout(60)`. Client payloads are stored with service_claim = epoch() at claim time (operations.rs:324) and service_port -1; after 60 s a client is returned to a new claimant as if it were a service. A service claimed 60 s ago is also re-claimable while its client is alive, so exclusivity lasts only 60 s (rmv's `service_claim = 0` reset at 668 implies exclusivity was intended). `current_epoch - v.service_claim` is u64 and can underflow if the client's clock is ahead (691).

**Fix:** Separate services from clients in State (or filter on service_port >= 0 / id == service_id), define lease semantics explicitly, use saturating_sub.

### P9. TCP connect-back race: retry sleeps not awaited, add() errors discarded

**Severity:** high  
**Category:** race  
**Location:** `src/service.rs:532`

`let _ = sleep(Duration::from_millis(1000));` creates but never awaits the future, so the 6 TcpStream::connect attempts (523-535) run back-to-back in microseconds. Over TCP the peer only binds its heartbeat server AFTER receiving the ACK (mode_tcp/operations.rs:112, operations.rs:430) while the broker's receive() already sent the ACK (connection.rs:271-276) before add() runs, so the connect-back frequently hits a closed port. The Err is thrown away (`let _ = state_loc.add(...)` at 766 and 822) so the peer is never registered and sits waiting for heartbeats forever, and for CLAIM the client has already been told the service address. Same un-awaited sleep at operations.rs:370 and service.rs:1184.

**Fix:** Await the sleep; propagate add() errors to the peer (NACK); or invert the handshake (peer binds first, then registers).

### P10. No message framing on raw TCP; back-to-back writes coalesce, 1024-multiple messages stall

**Severity:** high  
**Category:** wire-format  
**Location:** `src/connection.rs:212`

stream_read reads 1024-byte chunks until a chunk is shorter than 1024 (225-227). A message whose length is an exact multiple of 1024 causes an extra read that blocks 6 s and errors. Two JSON objects written back-to-back are returned as one string and fail to parse: the TCP CLAIM reply is exactly that (receive() writes {"header":"ACK","body":""} at 271-276, then request_handler writes the payload ACK at service.rs:821). Payloads carrying root_ca base64 certs are multi-KB and can arrive segmented (<1024 partial chunk -> truncated JSON -> unwrap panic).

**Fix:** Length-prefix or newline-delimit messages; keep a single reply per request.

### P11. HTTP ping-mode client pings with the SERVICE's ids, not its own

**Severity:** high  
**Category:** protocol-defect  
**Location:** `src/operations.rs:593`

claim --ping passes `service_payload` (the ACK body = the service's Payload) to ping_heartbeat (593-597), which extracts `service_id` and `id` from it (service.rs:1078-1080). The broker matches `e.service_id == hb_body.service_id && e.id == hb_body.id` (944), i.e. the SERVICE's Heartbeat, and refreshes its last_increment. The client's own entry is never refreshed and is marked GONE after 60 s (279-281) and removed; a dead ping-mode service is meanwhile kept alive by its client's pings.

**Fix:** Give the client its own (id, service_id) in the CLAIM ACK (e.g. return both payloads) and ping with those.

### P12. Process-global heartbeat/msg state and process::exit make the REST server single-tenant and self-terminating

**Severity:** high  
**Category:** architecture  
**Location:** `src/operations.rs:54`

GLOBAL_LAST_HEARTBEAT (operations.rs:54) and GLOBAL_MSGBODY (service.rs:69) are process-wide. In the api binary the REST front-end runs publish/claim inside the same process (api_builder.rs:218,350); all peers share one watchdog timer and one message slot. Watchdogs call std::process::exit(0) (operations.rs:576, mode_api/operations.rs:276), ping failure exits (service.rs:1171,1179) and TCP read errors exit (service.rs:1217-1229), so one peer's failure kills the whole REST server and every other registration in it. api_server also refreshes GLOBAL_LAST_HEARTBEAT for any GET /heartbeat_handler including `collect` COL requests (connection.rs:314-318).

**Fix:** Per-registration context struct passed through handlers; return errors instead of exiting.

### P13. Heartbeat period grows linearly with registered entities; peers self-terminate at ~30-50 entries

**Severity:** high  
**Category:** scalability  
**Location:** `src/service.rs:480`

event_monitor pops ONE Heartbeat per tick and sleeps 200 ms (480); an entry is re-queued only after its monitor completes. Per-entity HB period ~= max(N x 200 ms, RTT). TCP peers exit(0) when no bytes arrive within the 6 s stream_read timeout (service.rs:1223-1226, connection.rs:215) and HTTP peers exit when GLOBAL_LAST_HEARTBEAT is >10 s old (operations.rs:574). So >30 TCP or >50 HTTP registrations make peers kill themselves. Idle broker also spins at 200 ms toggling `running` (313-329,380-386).

**Fix:** Per-entity timers (tokio interval per Heartbeat) or a scheduler keyed by next-due time.

### P14. TCP CLAIM failure: NULL reply not awaited, stream dropped, client panics on EOF

**Severity:** high  
**Category:** protocol-defect  
**Location:** `src/service.rs:851`

`let _ = stream_write(&mut loc_stream, ...)` at 851-854 is never awaited so the NULL is not sent; the function returns Err and drops the stream. The client (operations.rs:369-391) is looping on stream_read waiting for a second ACK, gets Ok("") on EOF and deserialize_message panics (connection.rs:195). Over HTTP the same failure returns 400 with {"header":"NULL"} (860-864). Also the client's retry loop `let _ = sleep(...)` (operations.rs:370) is not awaited.

**Fix:** Await the write; define one NACK shape used by both transports; handle EOF as an error.

### P15. REST front-end returns before operations run and /collect never returns collected data

**Severity:** high  
**Category:** api-semantics  
**Location:** `src/api_builder.rs:482`

/publish and /claim spawn the long-running operation and reply "Successful request to publish/claim" immediately; the task's result is discarded (`let _ = Ok(task_response)` 245,376). /collect calls collect(), whose only output is println! (operations.rs:688,767); `result` is `()` and the body is overwritten at 503-505 even after an error was set at 495-500. /claim's payload is only printed to stdout (operations.rs:502). Bool params are compared as `Value == "true"` (221-222,231,353-354,362,483-484,609-610) so JSON booleans are ignored (`"print_v4": true` -> false -> v6 path -> only_or_error panic).

**Fix:** Make operations return data structures; REST handlers serialize them; parse booleans with as_bool().

### P16. Broker accept loop is serialized behind long retry loops in request_handler

**Severity:** high  
**Category:** liveness  
**Location:** `src/service.rs:885`

Because tcp_server awaits the handler inline (connection.rs:351), CLAIM retries (5 x 300 ms, 838-840), MSG lookup (50 x 300 ms = 15 s, 885-923) and ping HB lookup (50 x 702 ms = 35 s, 937-985) block all other TCP connections to the broker. add() TCP connect-back (522-536) also runs while holding the State lock (762), stalling the event loop. Over HTTP these loops instead hold a hyper task; ping senders time out at 10 s (1148) while the broker may take 35 s.

**Fix:** Spawn per connection; make lookups non-blocking (entries in flight should still be addressable, e.g. keep Heartbeats in a map and only queue ids).

### P17. --ping on the TCP binary registers a ping-mode entry nobody pings; removed after 60 s

**Severity:** medium  
**Category:** transport-divergence  
**Location:** `src/operations.rs:258`

Payload.ping = inputs.ping is sent on both transports (258,331) but ping_heartbeat is only started on the API path (mode_api/operations.rs:297-305, operations.rs:591-599). With TCP + --ping the broker creates hb.ping = true (617), never sends HB (143), and marks GONE when last_increment is >60 s old (279) -> removed. The TCP peer never receives anything and waits forever (no exit since the 6 s timeout only starts after a connection exists). Broker's TCP HB branch also writes no reply (966-974).

**Fix:** Reject --ping on TCP or implement a TCP ping sender; validate ping vs transport at registration.

### P18. Heartbeat.tls derived from root_ca presence, not from the peer's TLS mode

**Severity:** medium  
**Category:** transport-divergence  
**Location:** `src/service.rs:548`

`tls = p.root_ca.is_some()` (548-551) selects https:// vs http:// for the HB GET (216-229). A peer started with --tls but without --root_ca gets plain-HTTP heartbeats against a TLS acceptor; a peer with --root_ca but no --tls gets HTTPS against a plaintext port. The api CLI derives tls from --tls, the REST front-end from an https:// host (api_builder.rs:216,348), send/collect pick scheme by --tls ignoring Addr.transport (operations.rs:729-750,853-874), pings by the tls Option (service.rs:1123-1146) while registration uses the URL scheme (mode_api/operations.rs:181). TCP path ignores tls entirely; root_ca certs are still shipped in the payload.

**Fix:** Carry an explicit transport/scheme in the Payload (or use Addr) and derive everything from it.

### P19. event_monitor service_claim reset block is dead (fail_id already zeroed)

**Severity:** medium  
**Category:** dead-code  
**Location:** `src/service.rs:357`

Lines 354-355 set shared_data.0 = 0 and shared_data.4 = 0, then 357-370 search for `item.service_id == shared_data.4 as u64 && item.service_id == item.id`, i.e. a payload with id 0, which never exists. rmv() already resets service_claim (665-670) but picks the first payload with service_id == service_id, which may be another client rather than the service.

**Fix:** Delete the block; make rmv find the service by `id == service_id`.

### P20. TCP peer exits process on any read error on any accepted connection

**Severity:** medium  
**Category:** robustness  
**Location:** `src/service.rs:1215`

heartbeat_handler_helper spawns an infinite heartbeat_handler loop per accepted TCP connection (1017-1027), including ad-hoc send/collect connections and port scans. Any stream_read error (6 s idle, reset) calls std::process::exit(0) (1217,1221,1225,1229). EOF yields "" and deserialize panics the task (connection.rs:195). A slow `collect` caller or an idle probe on bind_port kills the service or client.

**Fix:** Distinguish the broker HB connection from ad-hoc request connections; handle EOF; never exit from a handler.

### P21. HTTP peer watchdog never fires if no heartbeat was ever received

**Severity:** medium  
**Category:** state-machine  
**Location:** `src/operations.rs:568`

The watchdog `continue`s while GLOBAL_LAST_HEARTBEAT is None (568-572; mode_api/operations.rs:268-272). Combined with the stubbed handler (finding 3) a peer whose registration silently failed (or whose broker died before the first HB) lives forever. TCP has no equivalent watchdog; its exit-on-6s-read-timeout only applies once the broker connected.

**Fix:** Start the timer at registration; unify a single watchdog policy across transports.

### P22. PUB ACK body differs: HTTP returns pre-add seq (wrong on reconnect), TCP returns empty

**Severity:** medium  
**Category:** transport-divergence  
**Location:** `src/service.rs:777`

HTTP PUB replies ACK{body: state_loc.seq} captured before add() (777,785) and mode_api::publish rewrites payload.id/service_id from it (mode_api/operations.rs:210-213). If add() takes the reconnect path it returns the OLD id and does not increment seq (581-584), so the publisher pings with a wrong id. TCP PUB gets only receive()'s generic ACK with empty body (connection.rs:271-276); mode_tcp::publish leaves id/service_id = 0 (mode_tcp/operations.rs:81-95).

**Fix:** Return the assigned Payload (post-add) as the ACK body on both transports.

### P23. Reply header asymmetry between transports (COL vs ACK, MSG echo vs none)

**Severity:** medium  
**Category:** transport-divergence  
**Location:** `src/service.rs:1394`

TCP COL reply uses header COL (1394-1397,1418-1421); HTTP COL reply uses ACK (1401-1407,1425-1431) and collect HTTP requires ACK (operations.rs:761-769) while collect TCP ignores the header (683-692). HTTP MSG relay replies ACK{body: msg} (1374-1380); TCP MSG writes nothing back so send_msg over TCP always waits the full 6 s and logs "Failed to collect message." (operations.rs:811-816). Broker HTTP MSG/HB replies echo the header (900-903,962-965); TCP writes nothing (904-912,966-974).

**Fix:** Define one response shape per request type independent of transport.

### P24. User message is JSON-encoded three times along send->collect

**Severity:** low  
**Category:** wire-format  
**Location:** `src/operations.rs:792`

send_msg sets body = serde_json::to_string(&inputs.msg) (792) -> "\"hello\""; heartbeat_handler stores that string as MsgBody.msg (1264/1297); COL reply body = serde_json::to_string(&msg.msg) (1396,1406) -> another layer; collect prints with {:?} (688,767) adding a fourth escaping.

**Fix:** Carry msg as a plain string field; print without Debug formatting.

### P25. Delivered message never cleared; re-sent in every HB

**Severity:** low  
**Category:** state-machine  
**Location:** `src/service.rs:149`

Heartbeat.msg_body set by MSG (898) is included in every subsequent HB (149-151,163-172) and the service overwrites GLOBAL_MSGBODY each time (1464-1465). Semantics: 'last message wins, delivered at-least-once forever'.

**Fix:** Clear msg_body after an acknowledged delivery or keep an explicit queue/sequence number.

### P26. Two-sided monitor never validates the HB reply

**Severity:** low  
**Category:** protocol-defect  
**Location:** `src/service.rs:262`

Any non-empty bytes count as alive (`received == ""` check 262-276); the header/body are not parsed. On TCP, EOF returns "" and is a failure; on HTTP `received` is never empty.

**Fix:** Parse and require header HB.

### P27. Reconnect matching by (service_addr, service_port) collides for all clients on one host

**Severity:** medium  
**Category:** state-machine  
**Location:** `src/service.rs:556`

Clients have service_port -1, so a second client from the same IP with the same key is treated as a reconnect of the first: the first's Heartbeat gets its stream/client replaced (575-579), the new Payload is never stored, the first peer's HB stream is dropped (its handler task panics on EOF).

**Fix:** Match reconnects on an explicit peer identity (id token returned at registration), not address/port.

### P28. Broker replies empty 200 when MSG/HB target not found; peers unwrap and panic

**Severity:** medium  
**Category:** protocol-defect  
**Location:** `src/service.rs:924`

After 50 retries the HTTP MSG (924-926) and HB (986-988) branches fall through to `Ok(response)` with an empty body and 200. ping_heartbeat does serde_json::from_reader(...).unwrap() (1157) and the HTTP MSG relay does the same (1344); heartbeat_handler HTTP relay panics after 5 failures (1363,1369). Pings time out at 10 s (1148) while the broker searches for 35 s.

**Fix:** Return a NULL/404-style message on not-found; never unwrap on responses.

### P29. API mode panics on socket-style broker address

**Severity:** low  
**Category:** input-handling  
**Location:** `src/operations.rs:435`

Addr::Display for Transport::SOCKET yields "host:port" (connection.rs:77); url::Url::parse("127.0.0.1:8000").unwrap() (operations.rs:435, mode_api/operations.rs:181) fails, so `api claim 127.0.0.1:8000` panics while the same string works for the tcp binary.

**Fix:** Validate transport per binary at CLI parse time.

### P30. GET /heartbeat_handler requests carry JSON bodies

**Severity:** low  
**Category:** wire-format  
**Location:** `src/service.rs:217`

Broker HB monitor (217-228) and collect (operations.rs:731-750) send GET with a request body; the router dispatches on path prefix only (connection.rs:311,314). Intermediaries may drop GET bodies.

**Fix:** Use POST (or put the discriminator in the path/query) in the rewrite.

### P31. Peer-controlled root_ca decoded with unwrap inside broker add()

**Severity:** medium  
**Category:** robustness  
**Location:** `src/tls.rs:111`

setup_https_client decodes each base64 line with `.unwrap()` (111); a malformed root_ca in a PUB/CLAIM payload panics the broker while it holds the State lock (service.rs:543). On TCP this kills the accept loop (finding 4).

**Fix:** Return an error and NACK the registration.

### P32. In-flight Heartbeats are invisible to MSG/HB/add lookups; all rely on polling retries

**Severity:** info  
**Category:** design  
**Location:** `src/service.rs:373`

Entries are removed from the deque while being monitored (373-376) and re-inserted only when the task finishes, so lookups by id (564,890,943) miss them; this is why the 10x, 50x300ms and 50x702ms retry loops exist and why they must hold locks repeatedly.

**Fix:** Keep Heartbeats in a HashMap<id, Heartbeat> owned by State and schedule by id.


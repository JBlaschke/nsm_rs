# Audit: Architecture and duplication

Findings reference `main` at commit `edd23a33` (2026-09-24), the state before the cleanup branches. Line numbers will drift as the cleanup lands; the file names are stable enough to locate each item.

Severity counts: 1 critical, 8 high, 7 medium, 2 low, 0 info.

## Summary

ARCHITECTURE/DUPLICATION lens of nsm_rs (4969 LOC, 2 bins, no lib). Verified against source with cargo clippy/build --offline.

Shape: two binaries (`src/tcp.rs`, `src/api.rs`) re-declare the identical 9-module tree and identical main() dispatch, differing only in the `ComType` literal passed to every operation. Transport selection is therefore decided by which binary is executed, even though `Addr.transport` (connection.rs:26-30, parsed at 93-135) already encodes SOCKET/HTTP/HTTPS and `api_builder.rs:216` already derives TLS from it. The "same" function then re-branches on `ComType` (30 sites) or on an `(Option<TcpStream>, Option<Request>)` pair (11 `panic!("Unexpected state...")` sites in service.rs), so every operation and handler is really two hand-written implementations interleaved in one body.

Module graph has three import cycles: connection <-> operations (via `GLOBAL_LAST_HEARTBEAT`, connection.rs:3/316 vs operations.rs:2-6), operations <-> mode_tcp and operations <-> mode_api (mode_* import `AMState`/`HttpResult`/`GLOBAL_LAST_HEARTBEAT` from operations while operations imports mode_*). `service` reaches `operations` transitively through `connection`. Only tls, network, utils, models are leaves.

Duplication inventory (details in extra): 26 duplicated blocks; largest are the 7-copy `HttpsConnectorBuilder`+`Client` construction, 6 http/https `Request::builder` pairs whose only delta is the scheme literal, the 3-copy hyper accept loop with optional `TlsAcceptor` (+1 plain copy in api.rs), the 2-copy `std::process::exit` heartbeat watchdog, the 4x4 JSON-parameter extraction matrix in api_builder.rs (~380 of its 640 lines), 11 identical `Response::builder().status(OK).header(CONTENT_TYPE).body(json)` literals, and ~250 lines of *dead* HTTP branches in `heartbeat_handler`/`heartbeat_handler_helper` (only reachable via commented-out code at mode_api/operations.rs:243-247 and operations.rs:535-541).

Structural defects that a refactor must design around (not just tidy): (1) process-wide `lazy_static` singletons (`GLOBAL_LAST_HEARTBEAT`, `GLOBAL_MSGBODY`) mean the REST front-end, which runs publish/claim as background tasks in ONE process (api_builder.rs:218, 350), shares a single "last heartbeat" and a single message slot across all sessions and any one watchdog `process::exit(0)` kills the whole server; (2) HTTP two-sided heartbeat is unimplemented (handlers reply 400 "Not implemented") and the broker unwraps the resulting non-JSON body (service.rs:236) so the monitor task panics and the entry is silently dropped from the deque; (3) core types leak transport: `Heartbeat` owns `Option<TcpStream>`/`Option<hyper Client>`, `event_monitor`/`Heartbeat::monitor` signal internal outcomes via HTTP `StatusCode`, all operations return `Result<Response<Full<Bytes>>, io::Error>` even for CLI/TCP; (4) `Message.body: String` is JSON-in-JSON with the discriminant separate from the payload; (5) TCP framing relies on `read() < 1024` (connection.rs:225) with no length prefix; (6) `event_monitor` shares one `(fail_count,key,id,service_id,fail_id)` tuple across all spawned tasks and calls `claim()` on a `State` clone (service.rs:392, 440) so the re-claim mutation is lost; (7) `api` binary's REST mode (api.rs:112-128) is unreachable because `OPERATION` is `required(true)` (cli.rs:21) -- confirmed by running `./target/debug/api` -> clap usage error; Docker CMD (Dockerfile:73) also uses the retired `--operation` flag; (8) `tls: bool` flag and `Addr.transport` are two sources of truth for TLS.

Keep: `Addr`/`Transport` parsing+Display, `MessageHeader` vocabulary, `models.rs` operation inputs (after factoring common fields), `network.rs`, `tls.rs` cert/key/root loaders, `FailCounter` + 10-failure/reclaim policy, `State::claim` timeout semantics, api_builder's parameter names. Must go: Option/Option pairs, `ComType`, both lazy_statics, two binaries + duplicate module lists, `HttpResult`/`StatusCode` as internal status, transport handles inside `Heartbeat`, string-body `Message`, both hand-rolled routers, the shared `data` tuple and `State: Clone`, `process::exit`/`panic!`/`println!` in library paths, `tls: bool`, 1024-byte framing, `threadpool`/`rustls-platform-verifier`/empty `ring`/`aws-lc-rs` features.

Proposed target: one package with `lib.rs` + `bin/nsm.rs`; layers core -> transport -> ops -> control_plane -> cli with strictly downward imports; `Link`/`Server` transport traits (tcp + http impls) chosen from `Addr.transport`; per-operation `Session` context replacing globals; axum control plane with a job registry; integration tests spawning broker+publish+claim over both transports on localhost.

## Detail

## 1. Module dependency graph (who imports whom)

```
tcp.rs (bin)  ──> cli, operations, connection::ComType   (declares: service models tls network mode_api mode_tcp connection utils cli operations)
api.rs (bin)  ──> cli, operations, connection::ComType, api_builder, hyper (own server loop)   (declares same 9 mods + api_builder)

cli.rs        ──> connection::Addr, models
models.rs     ──> connection::Addr
api_builder.rs──> operations::{get_interfaces,claim,publish,collect,send_msg}, models, network, connection::{Addr,ComType,Transport}

operations.rs ──> network, connection, service, utils, models, tls::{tls_config,load_ca}, mode_api, mode_tcp      (defines HttpResult, Sem, AMState, GLOBAL_LAST_HEARTBEAT)
mode_tcp/operations.rs ──> connection, service, operations::{AMState,HttpResult}                     [CYCLE with operations]
mode_api/operations.rs ──> connection, service, tls::{tls_config,get_tls_acceptor}, operations::{AMState,HttpResult,GLOBAL_LAST_HEARTBEAT}  [CYCLE with operations]

service.rs    ──> utils, connection, tls::setup_https_client
connection.rs ──> operations::GLOBAL_LAST_HEARTBEAT   (connection.rs:3, used :316)                   [CYCLE: connection -> operations -> connection]

tls.rs        ──> (leaf: rustls, hyper_rustls, hyper_util, base64)
network.rs    ──> (leaf: pnet)
utils.rs      ──> (leaf)
```

Cycles:
1. connection.rs:3 -> operations.rs:53 (GLOBAL_LAST_HEARTBEAT) while operations.rs:2-6 -> connection. Transitively service.rs:28 -> connection -> operations -> service (operations.rs:7).
2. operations.rs:17 -> mode_tcp; mode_tcp/operations.rs:5 -> operations::{AMState,HttpResult}.
3. operations.rs:16 -> mode_api; mode_api/operations.rs:9-10 -> operations::{AMState,HttpResult,GLOBAL_LAST_HEARTBEAT}.

Layering violations: lowest I/O module (connection) depends on the top-level ops module; core registry (service) depends on tls (service.rs:30 `setup_https_client` used at :543) and on hyper client types (service.rs:9-15, 122).

## 2. Duplication inventory

| # | Block | Copies (file:lines) | What varies |
|---|-------|---------------------|-------------|
| D1 | Module declaration list | tcp.rs:8-24; api.rs:8-25 | order; api.rs adds `mod api_builder` |
| D2 | main() operation dispatch | tcp.rs:53-91; api.rs:71-109 | `ComType::TCP` vs `ComType::API`; tcp.rs uses `worker_threads=20` (tcp.rs:41) |
| D3 | env_logger init | tcp.rs:45-48; api.rs:58-61 | none |
| D4 | hyper accept loop + optional `TlsAcceptor` (`match tls_acceptor { Some => accept then serve_connection, None => serve_connection }`) | mode_api/operations.rs:92-129 (listen); mode_api/operations.rs:308-344 (publish); operations.rs:602-638 (claim API); api.rs:117-127 (plain, no TLS) | flag name used for the redundant `if flag {acceptor.clone()} else {None}` (`tls`/`use_tls`/`inputs.tls`); eprintln capitalisation; api.rs uses `println!` and `?` on accept vs `.unwrap()` |
| D5 | `service_fn(move |req| { let h = Arc::clone(&handler); async move { let l = h.lock().await.clone(); api_server(req, l).await } })` | mode_api/operations.rs:83-89; mode_api/operations.rs:289-295; operations.rs:583-589 | none |
| D6 | Bind of heartbeat/listen server address | mode_api/operations.rs:74-79 (format+parse SocketAddr); mode_api/operations.rs:286 (`to_socket_tuple`); operations.rs:551-553,582 (format+parse); connection.rs:340-342 (format string) | 3 different ways to turn host+port into a bind target |
| D7 | `HttpsConnectorBuilder::new()...build()` + `Client::builder(TokioExecutor::new()).build(..)` | tls.rs:105-141 `setup_https_client`; mode_api/operations.rs:135-157 `get_https_connector`; operations.rs:459-474 (claim); operations.rs:709-724 (collect); operations.rs:834-849 (send_msg); service.rs:1085-1103 (ping_heartbeat); service.rs:1277-1294 (heartbeat_handler MSG relay) | input: base64 cert string vs `Option<&ClientConfig>` vs `bool + Option<ClientConfig>.unwrap()` vs `Option<ClientConfig>` clone; error: `?` vs `.unwrap()`; body type generic `T` vs `Full<Bytes>` |
| D8 | `ClientConfig::builder().with_root_certificates(load_ca(ROOT_PATH)).with_no_client_auth()` | tls.rs:149-158 (`get_tls_acceptor`); operations.rs:438-445 (claim); operations.rs:697-701 (collect); operations.rs:822-826 (send_msg) | ROOT_PATH optional (`match env::var`) vs mandatory (`expect("ROOT_PATH not set")`); claim additionally re-inlines `tls_config()` + `TlsAcceptor::from` (operations.rs:437, 451) = body of `get_tls_acceptor` (tls.rs:144-165); claim also computes an unused `_server_name` (operations.rs:446-450) |
| D9 | `Request::builder()` http-vs-https pair differing only in scheme literal | operations.rs:729-750 (GET /heartbeat_handler); operations.rs:853-874 (POST /request_handler); service.rs:216-229 (GET /heartbeat_handler, `self.addr`); service.rs:1123-1146 (POST /request_handler); service.rs:1312-1335 (POST /request_handler) | method/path/body; scheme is the only intra-pair delta. Singletons using url join (no pair): mode_api/operations.rs:192-197 and operations.rs:485-490 (identical to each other) |
| D10 | Send request with `timeout(6s/3s/10s)`, `collect_request(resp.body_mut()).await.unwrap()`, `match m.header {ACK/HB/MSG => info!, _ => warn!}` + `read_fail` retry | mode_api/operations.rs:199-233; operations.rs:492-525; operations.rs:752-774; operations.rs:876-890; service.rs:233-255; service.rs:1148-1182; service.rs:1337-1372 | expected header; threshold 5 vs 10; failure action panic!/process::exit/error!/warn!; service.rs:1154-1159 and 1343-1346 re-inline the body of `collect_request` (connection.rs:283-292) instead of calling it |
| D11 | Heartbeat-timeout watchdog (`sleep 5s; loop {sleep 500ms; if elapsed>10s {process::exit(0)}}`) on `GLOBAL_LAST_HEARTBEAT` | mode_api/operations.rs:257-280; operations.rs:557-580 | none (comment only) |
| D12 | Placeholder heartbeat handler returning 400 "Not implemented" | mode_api/operations.rs:238-255; operations.rs:530-549 | none |
| D13 | Event-monitor spawn | mode_tcp/operations.rs:47-53; mode_api/operations.rs:54-59 | `debug!` vs `trace!`; TCP awaits the JoinHandle, API drops it |
| D14 | `Box::pin(async move { ... }) as Pin<Box<dyn Future<Output=HttpResult> + Send>>` handler closures | mode_tcp/operations.rs:29-34, 103-109; operations.rs:412-425; mode_api/operations.rs:66-71, 238-255; operations.rs:530-549 | which handler is called; some wrapped in `Arc<Mutex<..>>` |
| D15 | certs -> base64 bundle (`fs::File::open(path)?; rustls_pemfile::certs(..).collect()?; map(encode).join("\n")`) | operations.rs:230-245 (publish); operations.rs:301-316 (claim) | none. Related: same PEM read in tls.rs:45-59 `load_ca`; decode counterpart tls.rs:109-120 |
| D16 | ipstr selection prologue (`get_local_ips`, `if print_v4 {get_matching_ipstr(v4..)} else {v6}`, `only_or_error`) | operations.rs:170-182 (listen); operations.rs:217-228 (publish, + `all_ipstr`); operations.rs:287-299 (claim, `_all_ipstr` unused); also v4/v6 print loops in list_interfaces operations.rs:96-116 and list_ips :130-156; api_builder.rs:78-89 | whether `all_ipstr` is computed/used |
| D17 | Payload construction | operations.rs:248-259 (publish); operations.rs:321-332 (claim) | `service_port` (real vs -1), `service_claim` (0 vs epoch()), `interface_addr` |
| D18 | JSON body collect + parse (`request.body_mut().collect().await` -> `aggregate()` -> `serde_json::from_reader`, two 400 error branches) | api_builder.rs:114-135; 257-278; 388-409; 514-535; (also connection.rs:282-294 `collect_request` with unwrap) | none |
| D19 | Parameter extraction `match data.get("X").and_then(..) { Some => .., None => 400 "Error: 'X' parameter is required." }` | `name`: api_builder.rs:139-148, 282-291, 412-421, 540-549; `host`+`Addr::from_str`: 150-170, 293-313, 423-443, 551-571; `bind_port`: 172-181, 315-324 (commented 446-455); `service_port`: 183-192; `key`: 194-203, 326-335, 457-466, 584-593; `msg`: 573-582; `starting_octets`: 205-208, 337-340, 469-472, 596-599; `root_ca`: 210-213, 342-345, 474-477, 601-604; `use_tls`: 216, 348, 480, 607; `print_v4/print_v6/ping` `map_or(.., |v| v == "true")`: 221-222, 231, 353-354, 362, 483-484, 609-610 | key name and type; `print_v6` default `true` in publish (:222) vs `false` elsewhere (:354, 484, 610); `v == "true"` only matches a JSON *string* "true", not boolean `true` |
| D20 | Query-string parse into HashMap | api_builder.rs:23-27; 58-62 | param names `v4/v6` vs `get_v4/get_v6` (inconsistent API) |
| D21 | Serialize `response_data` to JSON with 400 fallback | api_builder.rs:38-49; 93-104 | none |
| D22 | `tokio::spawn` operation + discarded `task_response` | api_builder.rs:218-246; 350-377 | operation |
| D23 | `Response::builder().status(StatusCode::OK).header(CONTENT_TYPE,"application/json").body(Full::new(Bytes::from(json))).unwrap()` | service.rs:779-782, 825-828, 860-863 (BAD_REQUEST), 906-909, 968-971, 1187-1193, 1374-1380, 1401-1407, 1425-1431, 1452-1458, 1476-1482 | status and body |
| D24 | `(stream, request)` write-response pair (`stream_write(serialize_message(..))` vs `Response::builder..`) | service.rs:764-794, 816-832, 848-866, 904-912, 966-974, 1390-1410, 1414-1434, 1441-1461, 1467-1485 | header/body; 11 `panic!("Unexpected state...")` fallbacks at service.rs:258, 738, 793, 831, 865, 1234, 1382, 1409, 1433, 1460, 1484 |
| D25 | Poll-the-deque loop (`while counter < N { lock state; lock deque; find_map by id; mutate; break } sleep`) | service.rs:553-598 (`add`, N=10, no sleep); service.rs:882-927 (MSG, N=50, 300ms); service.rs:934-989 (HB, N=50, 702ms) | predicate, N, sleep, mutation |
| D26 | `io::ErrorKind` match ladder (ConnectionReset/ConnectionAborted/TimedOut/_) | service.rs:175-208 (monitor: increment fail counter); service.rs:1210-1231 (heartbeat_handler: `process::exit(0)` in every arm) | action; the four arms are otherwise identical within each copy |
| D27 | TCP connect + `Arc<Mutex<TcpStream>>` + `send(msg)` + `match ack` | mode_tcp/operations.rs:69-100; operations.rs:347-407; operations.rs:666-692; operations.rs:800-816; service.rs:1255-1272 | expected header, error text |
| D28 | HB message serialized twice in `Heartbeat::monitor` | service.rs:149-152 (`message`) and service.rs:165-171 (rebuilt inline instead of using `message`) | none |
| D29 | `MsgBody { msg: message.body.clone(), id: 0, service_id }` | service.rs:1263-1267; 1296-1300 | none |
| D30 | crypto provider `install_default()` cfg blocks | tls.rs:81-84; operations.rs:651-654; operations.rs:785-788 | none (dead by default; features empty) |
| D31 | `Addr::new(&host, inputs.bind_port)` computed in both `match com` arms | operations.rs:266, 273 | none |

Dead code confirmed: `heartbeat_handler_helper` HTTP arm (service.rs:1030-1044) and all HTTP arms of `heartbeat_handler` (service.rs:1233, 1274-1381, 1399-1408, 1423-1432, 1450-1459, 1475-1483); `Sem` alias used once; `only_or_none`, `epoch` `#[allow(unused)]` (utils.rs:12, 21; epoch actually used); `State::new(_tls)` param (service.rs:501); `_server_name` (operations.rs:446); `_all_ipstr` (operations.rs:289); `tls::setup_https_client` and `mode_api::get_https_connector` coexist with TODOs pointing at each other (tls.rs:103, mode_api/operations.rs:134); `let _ = sleep(..)` never awaited (operations.rs:370, service.rs:532, 1184).

## 3. Proposed target layering (single library crate + one binary)

```
nsm/
  Cargo.toml            [lib] + [[bin]] nsm ; features: tls-aws-lc (default) | tls-ring
  src/lib.rs            pub mod core, transport, ops, control_plane; pub use core::*;
  src/core/             NO tokio-net / hyper / rustls imports; unit-testable
    addr.rs             Addr { transport: Transport, host: String, port: u16 } + FromStr/Display (KEEP from connection.rs:24-135; add url(path), socket_addr(), port u16)
    message.rs          #[serde(tag="type")] enum Message { Publish(PublishRequest), Claim(ClaimRequest), Ack(Ack), Heartbeat(HbBody), Collect, Msg(MsgBody), Null }  (replaces Message{header,body:String})
    registry.rs         Registry { entries: HashMap<EntryId, Entry>, by_key: HashMap<Key, Vec<EntryId>>, seq } ; add/remove/claim(timeout)/release/set_inbox ; Entry { id, service_id, key, kind: Service{service_addr,port}|Client, bind_addr: Addr, claimed_at: Option<Instant>, root_ca: Option<Vec<CertificateDer>>, ping: bool }  (replaces State.clients + Heartbeat field dupes)
    heartbeat.rs        FailCounter (KEEP semantics service.rs:72-105), HeartbeatPolicy { interval, max_fail:10, ping_timeout:60s }, enum HeartbeatOutcome { Alive, Failed, Gone } ; fn monitor<L: Link>(entry, link, policy) -> Outcome
    error.rs            #[derive(thiserror::Error)] enum NsmError { Io, Codec, Tls, Timeout, KeyNotFound, PeerGone, AddrParse, ... }
  src/transport/
    mod.rs              trait Link  { async fn call(&mut self, Message) -> Result<Message>; }   // client side, one request/response
                        trait Server{ async fn serve(self, addr: Addr, tls: Option<TlsServer>, handler: Arc<dyn Handler>) -> Result<()>; }
                        trait Handler { async fn handle(&self, route: Route, msg: Message) -> Result<Message>; }  // Route::{Broker, Heartbeat}
                        enum AnyLink { Tcp(TcpLink), Http(HttpLink) } ; async fn connect(addr:&Addr, tls:&TlsClient) -> AnyLink   // chosen by addr.transport
    codec.rs            serde_json encode/decode of Message; LengthDelimitedCodec framing for TCP
    tcp.rs              TcpLink (Framed<TcpStream, LengthDelimitedCodec>), TcpServer (spawn per connection; Route derived from first frame or fixed per listener)
    http.rs             HttpLink (ONE HttpClientBuilder from TlsClient), HttpServer (ONE accept loop with Option<TlsAcceptor>, axum Router: POST /v1/broker, GET|POST /v1/heartbeat)
    tls.rs              TlsSettings { cert: PathBuf, key: PathBuf, root: Option<PathBuf> } (from CLI/env once), fn server_config(), fn client_config(root: RootSource) ; KEEP load_certs/load_private_key/load_ca bodies from tls.rs:21-76, replace 4 ClientConfig copies and 7 client-builder copies
  src/ops/              written ONCE against Link/Server ; return Result<Outcome, NsmError> ; no println/panic/exit
    session.rs          Session { last_heartbeat: Mutex<Option<Instant>>, inbox: Mutex<Option<MsgBody>>, service: Mutex<Option<ServiceHandle>>, cancel: CancellationToken }   (replaces GLOBAL_LAST_HEARTBEAT, GLOBAL_MSGBODY, service_payload Arc<Mutex<String>>)
    broker.rs           listen(opts) : Registry actor (mpsc) + one heartbeat task per entry + Server with BrokerHandler (request_handler rewritten as fn(&Registry, Message)->Message)
    publish.rs          publish(opts, session): connect(host).call(Publish) -> Ack{id} ; spawn watchdog(session) ; Server(bind_addr, HeartbeatHandler(session)) or ping loop
    claim.rs            claim(opts, session): call(Claim) -> Ack(ServiceHandle) ; same heartbeat server
    collect.rs, send.rs connect(target).call(Collect | Msg) -> print at CLI layer
    netinfo.rs          list_interfaces/list_ips (KEEP network.rs; make fns sync)
    mod.rs              enum Operation { ListInterfaces(..), ListIps(..), Listen(..), Publish(..), Claim(..), Collect(..), Send(..) } ; async fn run(op) -> Result<Outcome>
  src/control_plane/    axum Router: POST /v1/publish|claim (202 + job_id), POST /v1/send, GET /v1/collect, GET /v1/interfaces, GET /v1/ips, GET /v1/jobs/{id}, DELETE /v1/jobs/{id}; JobRegistry { id -> JoinHandle, status, CancellationToken }; request structs = same serde structs as CLI (models.rs, KEEP names)
  src/bin/nsm.rs        clap derive: nsm <list-interfaces|list-ips|listen|publish|claim|collect|send|serve> ; #[command(flatten)] LocalIface, TlsOpts ; transport from --host scheme (listen/serve: --transport tcp|http)
  tests/
    addr.rs             FromStr/Display round trips incl. IPv6, trailing slash, bad port
    registry.rs         add/claim/timeout/reclaim-on-service-death/remove-empties-key
    framing.rs          messages of 1024*n bytes, split writes
    e2e_tcp.rs, e2e_http.rs   spawn broker + publish + claim + send + collect on 127.0.0.1 ephemeral ports, both transports, with and without TLS (rcgen self-signed)
```

Import direction (enforced by module visibility, optionally by `cargo-deny`/`cargo-modules`): bin -> control_plane -> ops -> transport -> core. `core` depends only on std, serde, thiserror, tokio::time (Instant). `ops` never names hyper/TcpStream.

Keep (with edits): `Addr`/`Transport`/`ParseAddrError` (connection.rs:24-135); `MessageHeader` vocabulary as enum variants; `models.rs` structs as the shared CLI+REST request types after factoring `LocalIface`/`TlsOpts`; `network.rs` whole; `tls.rs:21-76` loaders; `FailCounter`; `State::claim` timeout rule (service.rs:685-701); `rmv` reclaim rule (service.rs:657-671, event_monitor 438-467); api_builder parameter names (name/host/bind_port/service_port/key/msg/starting_octets/root_ca/ping) as the REST contract; `cli.rs` flag names.

Must go: `ComType` (connection.rs:139-143) and all 30 branches; `Option<stream>/Option<request>` signatures and 11 panics; `GLOBAL_LAST_HEARTBEAT`, `GLOBAL_MSGBODY`, `lazy_static`; second binary and duplicate module lists; `HttpResult`/`AMState`/`Sem` aliases and `StatusCode` as internal status; `Heartbeat.stream`/`.client`/`.tls`; `Message.body: String`; `api_server` and `handle_requests` hand routers and `Arc<Mutex<FnMut>>` handler wrapping; `event_monitor`'s shared `data` tuple, `State: Clone`, deque re-queue loop; all `process::exit`/`panic!`/`println!` in non-CLI code; `tls: bool` on inputs; 1024-byte framing and `send()`'s ACK special-casing (connection.rs:240-245, 266-269); `Payload.root_ca` base64 string; `threadpool`, `rustls-platform-verifier`, `pki-types`, `lazy_static`, empty `ring`/`aws-lc-rs` features; `mode_tcp/`, `mode_api/`, `api_builder.rs` as files (contents absorbed into transport/ and control_plane/).

Repo hygiene tied to the restructure (out of lens but blocking it): `docs/` (rustdoc for nonexistent bin `nsm`), `vendor/` (356MB; keep `.cargo/config.toml` vendoring only if offline builds are a hard requirement, else drop and use Cargo.lock), `nsm-dev-buildx-latest.tar`, `src/.DS_Store`, `.env` with another developer's cert paths are tracked; Dockerfile:73 and src/test_event_monitor.sh use retired `--operation`/`-o` syntax; compose.yaml:15 port 12000 vs api.rs:115 port 8080.

## Findings

Ids are `A` plus the finding number, in the order the reviewer reported them (not by severity).

| # | Severity | Category | Finding | Location |
|---|---|---|---|---|
| A1 | high | architecture | Two binaries duplicate the whole module tree; transport chosen by binary instead of by Addr | `src/api.rs:8` |
| A2 | high | architecture | Circular module dependencies: connection<->operations, operations<->mode_tcp, operations<->mode_api | `src/connection.rs:3` |
| A3 | critical | architecture | Process-wide lazy_static singletons break multi-session use in the REST server process | `src/operations.rs:53` |
| A4 | high | architecture | Option<stream>/Option<request> dual-parameter functions with 11 panic sites | `src/service.rs:728` |
| A5 | high | duplication | claim/collect/send_msg still monolithic with inlined TCP-vs-API branches; listen/publish half-extracted | `src/operations.rs:344` |
| A6 | high | architecture | HTTP types and StatusCode used as the internal status protocol of the transport-agnostic core | `src/service.rs:137` |
| A7 | high | architecture | HTTP two-sided heartbeat path is unimplemented and its dead code remains; broker panics on the placeholder reply | `src/mode_api/operations.rs:238` |
| A8 | medium | architecture | Message envelope is JSON-in-JSON with header separated from typed body | `src/connection.rs:181` |
| A9 | medium | architecture | TCP framing relies on read() returning fewer than 1024 bytes | `src/connection.rs:225` |
| A10 | high | architecture | State/event_monitor design: shared result tuple, State: Clone per event, duplicated registry fields | `src/service.rs:305` |
| A11 | high | architecture | REST control plane is unreachable in the api binary and Docker CMD uses retired flags | `src/api.rs:63` |
| A12 | medium | duplication | Two hand-rolled HTTP routers with the same shape | `src/connection.rs:299` |
| A13 | medium | architecture | api_builder handlers spawn long-lived operations and discard their result | `src/api_builder.rs:218` |
| A14 | medium | architecture | `tls: bool` duplicates `Addr.transport`; two sources of truth for TLS | `src/models.rs:62` |
| A15 | medium | architecture | Payload is simultaneously wire request, registry record and TLS trust-anchor carrier | `src/service.rs:34` |
| A16 | medium | architecture | Library-level code uses println!/eprintln!, panic! and std::process::exit for control flow | `src/service.rs:1217` |
| A17 | low | duplication | Operation input structs repeat the same six fields seven times | `src/models.rs:123` |
| A18 | low | architecture | Unused/vestigial dependencies and features | `Cargo.toml:14` |

### A1. Two binaries duplicate the whole module tree; transport chosen by binary instead of by Addr

**Severity:** high  
**Category:** architecture  
**Location:** `src/api.rs:8`

src/tcp.rs:8-24 and src/api.rs:8-25 declare the same nine `mod` items and src/tcp.rs:53-91 vs src/api.rs:71-109 contain the identical `match args { CLIOperation::... }` dispatch, differing only in `ComType::TCP` vs `ComType::API`. Cargo.toml:40-46 defines both `[[bin]]`s and there is no `lib.rs`, so every module compiles twice, clippy reports 124/154 duplicated warnings, no `#[cfg(test)]` unit tests can target a library, and rustdoc in docs/ was generated for a binary `nsm` that no longer exists. Meanwhile `Addr.transport` (src/connection.rs:26-30, parsed at 93-135) already encodes SOCKET/HTTP/HTTPS and `api_builder.rs:216` already derives TLS from it, so the binary-level switch is redundant with information every operation already receives.

**Fix:** Collapse to one package: `src/lib.rs` exporting the layers, one `src/bin/nsm.rs` with clap-derive subcommands. Select transport from `Addr.transport` of `--host` (or an explicit `--transport` flag for `listen`, which has no host). Delete `ComType`.

### A2. Circular module dependencies: connection<->operations, operations<->mode_tcp, operations<->mode_api

**Severity:** high  
**Category:** architecture  
**Location:** `src/connection.rs:3`

`src/connection.rs:3` imports `crate::operations::GLOBAL_LAST_HEARTBEAT` and uses it at :316 inside `api_server`, while `src/operations.rs:2-6` imports `connection::*`. `src/mode_tcp/operations.rs:5` and `src/mode_api/operations.rs:9-10` import `AMState`, `HttpResult`, `GLOBAL_LAST_HEARTBEAT` from `crate::operations`, while `src/operations.rs:16-17` imports `mode_api`/`mode_tcp`. `service.rs:28` imports `connection`, which imports `operations`, which imports `service` (operations.rs:7). The lowest-level I/O module therefore depends on the highest-level operation module, and the partially extracted per-transport modules depend on the module that is supposed to be their caller.

**Fix:** Move `HttpResult`/`AMState` type aliases and the heartbeat timestamp into a `Session`/context struct owned by the operation and passed into the server handler; make transport modules leaf modules that import only `core`.

### A3. Process-wide lazy_static singletons break multi-session use in the REST server process

**Severity:** critical  
**Category:** architecture  
**Location:** `src/operations.rs:53`

`GLOBAL_LAST_HEARTBEAT` (src/operations.rs:53-55, written at src/connection.rs:316, read at src/operations.rs:559-577 and src/mode_api/operations.rs:259-279) and `GLOBAL_MSGBODY` (src/service.rs:68-70, written at :1464-1465, read at :1388) are one-per-process. The REST control plane runs `publish`/`claim` as background tasks inside the single `api` server process (src/api_builder.rs:218-246, 350-377), so two published services in the same process share one `MsgBody` slot (messages for service A are collected by service B) and one heartbeat timestamp; the first watchdog to trip calls `std::process::exit(0)` (src/operations.rs:576, src/mode_api/operations.rs:276) and kills every other session and the REST server itself.

**Fix:** Replace both globals with a per-operation `Session { last_heartbeat: Arc<Mutex<Option<Instant>>>, inbox: Arc<Mutex<Option<MsgBody>>>, cancel: CancellationToken }` created by the operation and captured by its handler closure; the watchdog cancels the token instead of exiting.

### A4. Option<stream>/Option<request> dual-parameter functions with 11 panic sites

**Severity:** high  
**Category:** architecture  
**Location:** `src/service.rs:728`

`request_handler(state, stream: Option<Arc<Mutex<TcpStream>>>, request: Option<Request<Incoming>>)` (src/service.rs:728-731), `heartbeat_handler_helper` (:995-997) and `heartbeat_handler` (:1199-1202) each accept both transports and re-dispatch with `match (stream, request) { (Some,None)=>..., (None,Some)=>..., _ => panic!("Unexpected state: no stream or request.") }` at src/service.rs:258, 738, 793, 831, 865, 1234, 1382, 1409, 1433, 1460, 1484. `Heartbeat` likewise carries `stream: Option<..>` and `client: Option<..>` (:120-122) matched at :155-259. Each function body is two interleaved implementations; an invalid combination is a runtime panic instead of a type error, and the TCP variant of `heartbeat_handler_helper` spawns an infinite loop (:1017-1027) while the HTTP variant is one-shot, so the same signature has two incompatible lifetimes.

**Fix:** Introduce `trait Link { async fn recv(&mut self)->Result<Message>; async fn send(&mut self, Message)->Result<()> }` (or a request/response `async fn call`) with `TcpLink` and `HttpLink` impls; write `request_handler(state, msg: Message) -> Result<Message>` and `heartbeat_handler(session, msg) -> Result<Message>` once, transport-free; the server layer decodes/encodes.

### A5. claim/collect/send_msg still monolithic with inlined TCP-vs-API branches; listen/publish half-extracted

**Severity:** high  
**Category:** duplication  
**Location:** `src/operations.rs:344`

The refactor moved `listen`/`publish` into `mode_tcp`/`mode_api` (src/operations.rs:190-203, 264-279) but `claim` (src/operations.rs:344-640, ~300 lines), `collect` (:661-776) and `send_msg` (:796-892) still `match com { ComType::TCP => {...}, ComType::API => {...} }` inline. The API branch of `claim` (:432-638) is a near-verbatim copy of `mode_api::publish` (src/mode_api/operations.rs:161-345): TLS setup (operations.rs:436-455 vs tls.rs:144-165 `get_tls_acceptor`), client construction (:459-474 vs mode_api:135-157), request retry loop (:481-526 vs mode_api:189-234), placeholder handler (:530-549 vs mode_api:238-255), watchdog (:557-580 vs mode_api:257-280), accept loop (:602-638 vs mode_api:308-344). `ComType::` appears 30 times across the tree.

**Fix:** Finish the extraction in the opposite direction: instead of one module per transport, write each operation once against the `Link`/`Server` traits so `mode_tcp`/`mode_api` disappear and only `transport::{tcp,http}` remain.

### A6. HTTP types and StatusCode used as the internal status protocol of the transport-agnostic core

**Severity:** high  
**Category:** architecture  
**Location:** `src/service.rs:137`

`Heartbeat::monitor` returns `Result<Response<Full<Bytes>>, IoError>` and encodes outcomes as `StatusCode::OK/BAD_REQUEST/REQUEST_TIMEOUT/GONE/ACCEPTED` (src/service.rs:139-140, 180, 192, 269, 275, 280, 283) which `event_monitor` decodes at :401-421. `event_monitor` itself returns `Response` (:294), `tcp_server`'s handler must return `Response<Full<Bytes>>` (src/connection.rs:333-335), and `listen`/`publish`/`claim` return `HttpResult` (src/operations.rs:48, 164, 211, 285) even in TCP/CLI mode. `Heartbeat` owns `Option<Arc<Mutex<TcpStream>>>` and `Option<Client<HttpsConnector<HttpConnector>, Full<Bytes>>>` (src/service.rs:120-122). The registry/heartbeat core therefore cannot be compiled or unit-tested without hyper, tokio networking and rustls.

**Fix:** Define `enum HeartbeatOutcome { Alive, Failed(FailReason), Gone }` and `Result<Outcome, NsmError>` in core; `Heartbeat` stores a `PeerId`/`Addr` and the monitor asks a `LinkFactory` for a connection. Operations return `Result<OperationOutcome, NsmError>`; only the control plane maps to HTTP.

### A7. HTTP two-sided heartbeat path is unimplemented and its dead code remains; broker panics on the placeholder reply

**Severity:** high  
**Category:** architecture  
**Location:** `src/mode_api/operations.rs:238`

In API mode the publish and claim heartbeat servers install a handler that returns 400 'Not implemented' for every request (src/mode_api/operations.rs:238-255, src/operations.rs:530-549); the real handler call is commented out (mode_api:243-247, operations.rs:535-541). Consequently `heartbeat_handler_helper`'s `(None, Some(_r))` arm (src/service.rs:1030-1044) and every HTTP arm in `heartbeat_handler` (:1233, 1274-1381, 1399-1408, 1423-1432, 1450-1459, 1475-1483; ~250 lines) are unreachable. On the broker side `Heartbeat::monitor` does `collect_request(resp.body_mut()).await.unwrap()` (src/service.rs:236); the 400 body 'Not implemented' is not JSON, `collect_request` returns Err (src/connection.rs:285-290), the unwrap panics inside the `tokio::spawn` at :397, and the Heartbeat is never pushed back to the deque while its `Payload` stays in `state.clients`. HTTP mode therefore only functions with `--ping`. `GLOBAL_LAST_HEARTBEAT` is nonetheless refreshed at src/connection.rs:316 before the 400 is produced, so the watchdog is fooled.

**Fix:** Make the heartbeat handler a transport-free `fn(session, Message)->Message` and mount it on both servers; delete the dead HTTP arms; never `unwrap` a peer-supplied body in the monitor (treat parse failure as a failed heartbeat).

### A8. Message envelope is JSON-in-JSON with header separated from typed body

**Severity:** medium  
**Category:** architecture  
**Location:** `src/connection.rs:181`

`Message { header: MessageHeader, body: String }` (src/connection.rs:180-186) carries a second serialized document in `body`: a `Payload` (src/service.rs:746-747, src/operations.rs:334-337), a `MsgBody` (src/service.rs:151, 877, 929), a bare `u64` seq (src/service.rs:777), or a JSON-encoded string of a string (src/operations.rs:792, src/service.rs:1396). Consumers must `match header` and then re-parse (src/service.rs:743-751) and every mismatch is an `unwrap` panic (src/connection.rs:190, 195; src/service.rs:715, 720). A transport abstraction cannot be typed while the wire format is stringly typed.

**Fix:** `#[derive(Serialize, Deserialize)] #[serde(tag="type")] enum Message { Publish(Payload), Claim(ClaimRequest), Ack(AckBody), Heartbeat(MsgBody), Collect, Msg(MsgBody), Null }` and a `Codec` (serde_json) shared by both transports.

### A9. TCP framing relies on read() returning fewer than 1024 bytes

**Severity:** medium  
**Category:** architecture  
**Location:** `src/connection.rs:225`

`stream_read` (src/connection.rs:212-231) appends 1024-byte reads and terminates on `bytes_read < buf.len()`. A message whose length is an exact multiple of 1024, a sender that writes in several segments, or two back-to-back messages (as `send` then `stream_read` on the same shared stream at :238-247 and the heartbeat loop at src/service.rs:1017-1027 do) will be mis-framed; `std::str::from_utf8(..).unwrap()` at :222 panics on a multi-byte char split across reads. Any `TcpLink` implementation needs a real frame boundary before the HTTP and TCP paths can share a `Link` trait.

**Fix:** Use `tokio_util::codec::{LengthDelimitedCodec, Framed}` (or newline-delimited JSON) in `transport::tcp`; expose `Link::call(Message)->Message` on top.

### A10. State/event_monitor design: shared result tuple, State: Clone per event, duplicated registry fields

**Severity:** high  
**Category:** architecture  
**Location:** `src/service.rs:305`

`event_monitor` communicates results of concurrently spawned monitor tasks through a single `Arc<Mutex<(i32,u64,u64,u64,i64)>>` (src/service.rs:305, written at :425-432, read at :332-370 on the next iteration) so results of one task can be overwritten by another before being consumed; `service_id: i64` (:306) is a loop-local latch. Each event clones the entire `State` (`state_loc.clone()` :392, `State: Clone` :485) and the re-claim `state_clone.claim(hb.key)` (:440) mutates the clone, so `service_claim` on the real registry is not updated. `Heartbeat` duplicates `key/id/service_id` from `Payload` (:109-117 vs :34-54) and the registry is `HashMap<u64, Vec<Payload>>` plus a separate `Arc<Mutex<VecDeque<Heartbeat>>>` (:488, 493) kept in sync by linear `find_map` scans (:564-572, 890-897, 943-958). `State::new(_tls: Option<ClientConfig>)` ignores its argument (:501).

**Fix:** Core `Registry { entries: HashMap<EntryId, Entry>, by_key: HashMap<Key, Vec<EntryId>>, seq }` with `add/remove/claim/reclaim/set_inbox` methods and unit tests; one `tokio::spawn`ed heartbeat task per entry that reports `HeartbeatOutcome` over an `mpsc` channel to a single owner of the registry (actor pattern), removing the deque, the shared tuple and `Clone`.

### A11. REST control plane is unreachable in the api binary and Docker CMD uses retired flags

**Severity:** high  
**Category:** architecture  
**Location:** `src/api.rs:63`

`api.rs:63` chooses REST mode when `!matches.contains_id("operation")`, but `cli.rs:17-21` declares `OPERATION` positional with `.required(true)`, so `init()` (`get_matches()`, cli.rs:136) exits with a usage error before line 63 is reached. Verified: `./target/debug/api` prints 'error: the following required arguments were not provided: <OPERATION>'. The REST server at api.rs:112-128 (hardcoded 0.0.0.0:8080, log typo '0.0.0.0.1:8080' at :114) cannot be started. Dockerfile:73 `CMD [... "--operation", "listen", ...]` and src/test_event_monitor.sh:5-7 use the old `--operation`/`-o` flag form that cli.rs no longer defines, so the image and the script are also broken; compose.yaml:15 exposes 12000 while api.rs binds 8080.

**Fix:** Make `serve` an explicit subcommand (`nsm serve --bind 0.0.0.0:8080`) in the single binary; update Dockerfile/compose/test script; add an integration test that starts the control plane.

### A12. Two hand-rolled HTTP routers with the same shape

**Severity:** medium  
**Category:** duplication  
**Location:** `src/connection.rs:299`

`connection::api_server` (src/connection.rs:299-327) and `api::handle_requests` (src/api.rs:131-168) both do `match (method, path.as_str()) { (Method::X, p) if p.starts_with("/...") => handler(request).await, _ => 404 }`. `starts_with` accepts `/publishfoo`; the data-plane routes (`/request_handler`, `/heartbeat_handler`) and the control-plane routes are unrelated to each other and each handler re-implements body collection/JSON parsing (src/api_builder.rs:114-135 x4; src/connection.rs:282-294). Each route handler is wrapped as `Arc<Mutex<closure>>` and locked per request (src/mode_api/operations.rs:66-71, 83-89, 238-255, 289-295; src/operations.rs:530-549, 583-589), serializing all requests through one mutex.

**Fix:** One `axum::Router` in `transport::http` for the data plane (`POST /v1/broker`, `GET /v1/heartbeat`) and one in `control_plane` for REST, with `Json<T>` extractors on serde-derived request structs and shared `State<Arc<Session>>`; drop the `Arc<Mutex<FnMut>>` wrapper.

### A13. api_builder handlers spawn long-lived operations and discard their result

**Severity:** medium  
**Category:** architecture  
**Location:** `src/api_builder.rs:218`

`handle_publish` and `handle_claim` `tokio::spawn` the operation (src/api_builder.rs:218-246, 350-377), build a `task_response` that is immediately dropped (`let _ = Ok::<..>(task_response)` :245, :376) and return 'Successful request to publish/claim' (:248, :379) before anything happened. There is no job id, no status endpoint, no cancellation; failures (panics at src/operations.rs:509-522, `process::exit` at :576) are invisible to the caller or fatal to the server. `handle_collect` overwrites its own error body with a success body (:492-505).

**Fix:** Control plane keeps a `JobRegistry { id -> JoinHandle + status + CancellationToken }`; `POST /publish` returns 202 with `{job_id}`, `GET /jobs/{id}` reports status/outcome, `DELETE /jobs/{id}` cancels.

### A14. `tls: bool` duplicates `Addr.transport`; two sources of truth for TLS

**Severity:** medium  
**Category:** architecture  
**Location:** `src/models.rs:62`

Every input struct carries `tls: bool` (src/models.rs:62, 100, 141, 171, 201) set from the global `--tls` flag in cli.rs:177, while `api_builder.rs:216, 348, 480, 607` compute it as `host_addr.transport == Transport::HTTPS`. `Heartbeat.tls` is a third copy derived from `root_ca.is_some()` (src/service.rs:548-551). `mode_api::publish` has a `// TODO: Infer TLS usage from Addr?` (src/mode_api/operations.rs:160). The http/https `Request::builder` pairs exist only because callers format the scheme by hand instead of using `Addr`'s `Display` (src/connection.rs:74-82).

**Fix:** Remove `tls` from models; derive scheme from `Addr.transport`; give `Addr` `fn url(&self, path:&str)->Url` and `fn socket_addr(&self)->Result<SocketAddr>`; the listen side takes `--tls` only for its own server certificate.

### A15. Payload is simultaneously wire request, registry record and TLS trust-anchor carrier

**Severity:** medium  
**Category:** architecture  
**Location:** `src/service.rs:34`

`Payload` (src/service.rs:34-54) is sent by publish/claim (src/operations.rs:248-259, 321-332) but contains broker-assigned fields (`id`, `service_id`, `service_claim`, `interface_addr`) that the client fills with zeros, plus `root_ca: Option<String>` holding a newline-joined base64 DER bundle read from disk (src/operations.rs:230-245, 301-316) and decoded by the broker into a `RootCertStore` (src/tls.rs:109-120). The same struct is echoed back to the client in ACK (src/service.rs:812-815) and stored verbatim in `state.clients`. `service_port: -1` is used as a client marker (src/operations.rs:323).

**Fix:** Split into `PublishRequest`, `ClaimRequest`, `ServiceRecord` (registry) and `ServiceHandle` (what a client receives); carry trust anchors as `Vec<CertificateDer>` in a dedicated field or, better, out-of-band via `--root-ca` on the broker.

### A16. Library-level code uses println!/eprintln!, panic! and std::process::exit for control flow

**Severity:** medium  
**Category:** architecture  
**Location:** `src/service.rs:1217`

`std::process::exit(0)` at src/service.rs:1171, 1179, 1217, 1221, 1225, 1229, src/operations.rs:576, src/mode_api/operations.rs:276; `panic!` for expected network failures at src/operations.rs:509, 516, 522, 686, 765, 772, 773, src/mode_api/operations.rs:224, 230, src/service.rs:1363, 1369; user-facing `println!` inside handlers and the registry at src/service.rs:646, 707, 770, 872, src/operations.rs:379, 502, 688, 767, src/mode_api/operations.rs:200, src/api.rs:124, 144. This prevents the operations from being hosted in one process (REST server) and from being tested.

**Fix:** Return `Result<_, NsmError>` (thiserror) everywhere; operations yield an `Outcome` value that the CLI layer prints; use `log`/`tracing` only.

### A17. Operation input structs repeat the same six fields seven times

**Severity:** low  
**Category:** duplication  
**Location:** `src/models.rs:123`

`print_v4`, `print_v6`, `name`, `starting_octets`, `tls`, `root_ca` are repeated in `Listen` (src/models.rs:50-65), `Claim` (:84-105), `Publish` (:123-146), `Collect` (:157-174), `SendMSG` (:185-204); `Collect`/`SendMSG` do not need interface selection at all (api_builder.rs:411, 468, 539, 595 TODOs). cli.rs:181-348 repeats the extraction of each of these per operation, and api_builder.rs repeats it per handler.

**Fix:** `struct LocalIface { ip_version: IpVersion, name: Option<String>, starting_octets: Option<String> }` and `struct TlsOpts { root_ca: Option<PathBuf> }` composed into operation structs; derive clap (`#[command(flatten)]`) and serde on the same structs so CLI and REST share one definition.

### A18. Unused/vestigial dependencies and features

**Severity:** low  
**Category:** architecture  
**Location:** `Cargo.toml:14`

`threadpool` (Cargo.toml:14) is never imported (only mentioned in comments src/service.rs:396, 484). `rustls-platform-verifier` (Cargo.toml:22) is optional and no feature enables it. `[features] ring = [] aws-lc-rs = []` (Cargo.toml:35-37) are empty and do not forward to `rustls/ring`; the `#[cfg(feature="ring")] install_default()` blocks (src/tls.rs:81-84, src/operations.rs:651-654, 785-788) are dead by default and would fail to compile if `ring` were enabled since hyper-rustls 0.27.5 only enables `rustls/aws_lc_rs` (vendor/hyper-rustls/Cargo.toml default features). `lazy_static` should be `std::sync::LazyLock`/removed. `pki-types` renamed dep used once (src/tls.rs:4) though `rustls::pki_types` is re-exported (src/operations.rs:41).

**Fix:** Remove threadpool, rustls-platform-verifier, lazy_static, pki-types, the empty features and all `install_default` blocks; pick one provider explicitly (aws-lc-rs via hyper-rustls default, or `ring` via features forwarded to rustls/hyper-rustls/tokio-rustls).


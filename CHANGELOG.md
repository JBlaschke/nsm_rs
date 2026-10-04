# Changelog

All notable changes to NSM are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

The 2026-09 cleanup rewrote the backend once for every transport. Everything
below compares against the last pre-cleanup state of `main`. Wire protocol
version: 4.

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
- **Service handles** (the claim reply, `peer` on a client, REST job views)
  no longer contain the rendezvous key.
- **`collected` names the answering party.** The reply to `collect` carries
  `role` (`service` or `client`), so the asker knows which field applies
  instead of guessing from what is set; this is wire protocol version 2.
  `POST /v1/collect` answers `{"role":"service","text":...}` or
  `{"role":"client","service":...,"text":...}` instead of an untagged object.
- **`deliver` names no target** (wire protocol version 3). The broker
  delivers to the sender's peer as it knows it, so a text that races a
  re-pairing reaches the new service instead of being refused, and a service
  can relay text too.
- **The rendezvous key is text** (wire protocol version 4). `--key` on
  `publish`, `claim` and `store` takes 1 to 64 characters from
  `A-Z a-z 0-9 . _ - : /`, not starting with `-`: one shell word, the rule
  of a store key with a shorter cap; anything else is a usage error (exit
  2) or, on the control plane, a 400. Two keys are the same when their
  text is, so `1234` and `01234` are different keys. On the wire the key
  (`key` of `publish` and `claim`, `rendezvous` of `store_by_key`,
  `nsm_key` of `nsm_mesh_data`, the key fields of the status document and
  of a job view) is a JSON string; an unsigned integer, what parties of
  version 3 sent, still decodes as its decimal text, so a party or a
  script from before this change keeps working against a new broker and
  names the same keys. In the library: `protocol::Key` is a validated
  newtype (`FromStr`, `TryFrom<String>`, `From<u64>` for the decimal
  text) with `KeyError` and `MAX_KEY_BYTES`, and `Error::NoService`
  carries a `Key`. Decisions D29 to D31.
- `collect` and `send` ignore `--key` (still accepted, hidden).
- **`collect` answers one question.** It prints the last text a party
  received and nothing else. The service a client is paired with is now
  `nsm peer`. Scripts that collected an address from a client switch to
  `peer`.
- **Logging** uses `tracing`; `NSM_LOG_LEVEL` and `NSM_LOG_STYLE` keep their
  meaning, `--log-level` overrides them, logs go to stderr and stdout carries
  only a command's result.

### Added

- A license: the BSD 3-Clause License (`LICENSE`, `license` field in
  `Cargo.toml`). The repository had none before.
- A library crate (`nsm`) with the binary as a thin front-end; every operation
  is a typed function in `nsm::ops`.
- `nsm claim` prints one stdout line per pairing: the first at registration
  and one more each time the broker re-pairs the client after its service
  went away, so the last line is always the current service. The library
  exposes the same stream as `Session::pairings`.
- `nsm peer PARTY`: the service a client is paired with, as `host:port`, and
  nothing else; asked of a service it fails with a message naming the role.
  In the library, `Collected::text` and `Collected::service` are the two
  accessors behind `collect` and `peer`, and `Error::WrongRole` is how `peer`
  refuses a service.
- Text flows both ways: `nsm send` to a service's heartbeat address hands a
  text to the client holding it, delivered on the client's next heartbeat
  and read with `nsm collect` at the client (`POST /v1/send` and
  `POST /v1/collect` likewise). A client's `collect` answer carries both its
  pairing and its last text.
- A shared store at the broker, one per claim, which the command line and
  the control plane reach through `nsm store` and `POST /v1/store` (below).
  Three new messages carry it:
  `store` (operator to party), `store_relay` (party to broker, with its id
  and token) and `stored`, the reply naming the claim's client, the store's
  revision and the entries; they are new variants, so the wire protocol
  stays version 3. The store is created empty when a claim is granted, kept
  across re-pairings and dropped with the client; a service reaches it only
  while it holds the claim. Store keys are 1 to 128 characters from
  `A-Z a-z 0-9 . _ - : /`, not starting with `-`; versions come from one
  counter for the broker's whole life. In the library: `protocol::StoreKey`,
  `StoreOp`, `StoreEntry`, `Stored`, `broker::Store` and `Registry::store`.
- Both parties relay `store` to the broker as `store_relay`, adding their
  own id and token, and pass the `stored` reply back unchanged, so an
  operator reaches the claim's store through either party's bind address,
  in ping mode too, without ever holding a token. A party that has not
  registered yet refuses it itself, as it does `send`. In the library:
  `ops::store`, which returns the `Stored` reply (a refusal is
  `Error::Rejected`), with `StoreEntry`, `StoreKey`, `StoreOp` and `Stored`
  re-exported from `ops`.
- `nsm store get|put|delete|list PARTY [KEY]`: the shared store from job
  scripts, through either party's bind address. `get` prints the value and
  exits 3 when the key is not set; `put --value TEXT` prints the write's
  version; `delete` prints nothing on stdout, says on stderr whether it
  removed anything and succeeds either way; `list` prints the keys, one per
  line. `--json` prints the broker's reply as one line instead, with the exit
  status unchanged. An invalid store key is a usage error (exit 2). In the
  library: `cli::StoreCommand`.
- `POST /v1/store` on the control plane: `{"party":...,"op":"get"|"put"|
  "delete"|"list","key":...,"value":...}`, answered with the broker's reply
  (`{"client":...,"revision":...,"entries":[...]}`); an unset key is 200 with
  no entries, a refusal 400, an unreachable party 502. In the library:
  `rest::StoreBody`.
- Conditional store writes: a put or a delete may carry `if_version`
  (`--if-version N` on `nsm store put` and `nsm store delete`), applied only
  if the key is at version N, or only if it is not set when N is 0. The
  broker compares in the same step as the write, so two writers that read
  the same version cannot both get through, and a counter both parties
  increment loses no update. A write whose condition does not hold changes
  nothing and is answered with `stored` carrying `applied: false` and the
  key's current entry; `nsm store` then exits 4 with `nsm: KEY is at version
  V` or `nsm: KEY is not set` on stderr, and `POST /v1/store` answers 409
  with the reply and an `error` field. `applied` is on every `stored` reply
  (and on `--json` output) and decodes as true when absent; `if_version`
  decodes as none when absent; the wire protocol stays version 3. In the
  library: `StoreOp::if_version`, `Stored::applied`,
  `Stored::not_applied_reason` and `broker::store::Outcome`.
- `--max-store-bytes` on `listen` (default 16384, allowed 256 to 32768): the
  budget of each store, counting every entry as its JSON-encoded key and
  value plus 64 bytes. `listen` refuses to start when a full store's reply
  (the budget plus 4096 bytes, which cover the reply's own fields and the
  broker's reserved entries a `list` carries) would not fit
  `--max-frame-bytes`, so a broker's frame limit below 20480 bytes now needs
  a smaller store budget too, and one below 4352 bytes (the smallest budget
  plus 4096) can no longer start a broker at all.
- Exit code 3: the party answered but has nothing to report yet (`collect`
  before the first text, `peer` before the pairing, `store get` of a key
  that is not set), distinct from a failed operation (1) and a usage error
  (2).
- Exit status 4: a `store put` or `store delete` with `--if-version` was
  answered but not applied, because the key was not at the version it named.
- Four transports from one implementation: TCP, TCP+TLS (`tls://`, new),
  HTTP, HTTPS.
- Registration tokens: 128-bit secrets issued at registration and required on
  pings, relayed messages and the broker's heartbeats.
- Broker admission policy: `--max-registrations-per-host` (default 64) and
  `--require-matching-host`.
- Flags for every timing and limit: `--heartbeat-interval`,
  `--heartbeat-timeout`, `--fail-threshold`, `--ping-staleness`,
  `--broker-watchdog`, `--request-timeout`, `--connect-timeout`,
  `--max-frame-bytes`, `--max-connections`, `--max-registrations`,
  `--max-store-bytes`.
- `--bind-port` is optional on `publish` and `claim`: left out, or given
  as 0, the operating system picks a free port for the party's heartbeat
  listener, and the party prints the address it bound on stderr. `listen`
  still requires it: the broker is the one fixed address. Decision D25.
- The broker's own store keys, through `nsm store` and `POST /v1/store`:
  where the parties of the claim listen, built from the broker's registry
  when asked and never stored. `nsm_mesh_data` is all of it as one JSON
  value: `nsm_service_address` and `nsm_service_port` (the service's
  data-plane endpoint), `nsm_mesh_service_address` and
  `nsm_mesh_service_port` (its heartbeat address), `nsm_mesh_client_address`
  and `nsm_mesh_client_port` (the client's), each endpoint also as one
  string (`nsm_service`, `nsm_mesh_service`, `nsm_mesh_client`), with
  `nsm_key` and both ids, `null` for a side that is not there; and every
  one of those fields is a key of its own, so `nsm store get PARTY
  nsm_mesh_client_port` prints the port, and a field that is `null` is a
  key that is not set (exit 3). They are entries of version 0 that `list`
  shows beside the stored ones. Store keys starting with `nsm_` are
  reserved from now on: a put or a delete of one is refused (exit 1, HTTP
  400). In the library: `protocol::MeshData` with `entries`,
  `StoreKey::is_reserved`, `StoreKey::mesh_data`, `Stored::mesh_data` and
  `Registry::mesh_data`. Decisions D25 and D26.
- `store_by_key`: a store operation addressed to the broker by rendezvous
  key, for a script that knows the key and the broker's address but not
  where the parties listen. The broker resolves the key to its one claim
  (or to its one service while nobody holds it), or to the party
  `party_id` names, and answers as for a relay; an unknown or ambiguous
  key is refused with the candidates listed. On the command line
  `nsm store get|put|delete|list ADDR [STORE_KEY] --key RENDEZVOUS
  [--party-id ID]`, where `ADDR` is then the broker's address (the usage
  lines now say `<ADDR>` and `<STORE_KEY>`); on the control plane
  `POST /v1/store` with `broker` and `rendezvous` (and `party_id`) in place
  of `party`. No token travels: the rendezvous key is the capability, as
  for `publish` and `claim`. One more variant, which needed no protocol
  bump; `nsm_requests_total` gains `kind="store_by_key"`, and
  operations by key count in `nsm_store_ops_total`. In the library:
  `Message::StoreByKey`, `Registry::resolve_key` and
  `Registry::store_by_key`, `ops::StoreTarget` and `ops::store_by_key`,
  `cli::StoreWhere`, `rest::StoreBody::target`. Decisions D27 and D28.
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
- The broker counts what it does (decisions D21 and D22):
  requests by kind and outcome, registrations granted and refused by
  reason, removals by role and reason, re-pairings, heartbeats by outcome
  with a round-trip histogram, and store operations by operation and
  outcome; and it reads the current state (parties by role and mode,
  unclaimed services, failing parties, stores and their bytes, per key and
  per host) from the registry on demand. In the library:
  `broker::metrics` with `Metrics`, `Gauges`, the Prometheus text
  exposition `Metrics::render`, and `Status`, the JSON view;
  `Broker::metrics`, `gauges`, `render_metrics` and `status`;
  `Broker::drop_party` takes a `RemovalReason` instead of free text.
- `nsm listen --admin-bind ADDR [--admin-token TOKEN]` (`NSM_ADMIN_TOKEN`):
  an admin listener on a second, plain-HTTP socket with `GET /metrics` (the
  Prometheus text exposition), `GET /v1/status` (one JSON document: version,
  uptime, limits and timing in force, the current counts with a per-key and
  a per-host breakdown, the counters since start, every party) and
  `GET /healthz`. Off unless asked for; loopback needs no token, any other
  address requires one, checked on every request in constant time, as for
  `nsm serve` (decision D9). Parties never use it. The broker prints
  `nsm: admin listener on http://ADDR` on stderr. Documented in
  `docs/MONITORING.md`. In the library: `broker::admin` (`AdminOpts`,
  `serve`, `router`), `ListenOpts::admin`, `ListenRequest::admin`,
  `BrokerHandle::admin_addr` and `BrokerHandle::status`;
  `cli::AdminListenerOpts`. Decision D23.
- `nsm status ADMIN [--json] [--parties] [--watch SECS] [--admin-token
  TOKEN]`: a broker's usage statistics from its admin listener, as one
  block (the broker and its uptime, parties by role, key and host, stores,
  and the counters since start), with one row per party on `--parties`,
  the status document as one JSON line on `--json`, and repeated with a
  timestamp header on `--watch`. Exit 1 when the listener cannot be reached
  or refuses the token, 2 for a `tls://` or `https://` address. In the
  library: `ops::status` and `Status::summary`. Decision D22.
- A local monitoring stack: `deploy/monitoring/compose.yaml` runs
  Prometheus and Grafana on loopback with the datasource and the `NSM
  broker` dashboard provisioned (`grafana/dashboards/nsm.json`), scraping a
  broker on the host through `host.docker.internal` with the token from
  `NSM_ADMIN_TOKEN_FILE` (default `admin-token.example`), or a broker
  inside the stack with `--profile broker`. `scripts/monitoring-local.sh
  fetch|start|status|stop` runs the same two servers without containers,
  as the current user, from one work directory, for interactive HPC nodes:
  `fetch` downloads the release tarballs and checks their SHA-256 sums.
  Decision D24.

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
  [#3](https://github.com/JBlaschke/nsm_rs/issues/3)).
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
- A closed stdout (a reader that went away, as in `nsm list-interfaces |
  head -0`) made `nsm` panic with exit 101; it now exits 1 with an `nsm: `
  message. `nsm claim` exits 1 the same way when its first line cannot be
  written. Its re-pairing lines, which printed a panic message when the
  reader had gone (the party kept running), now stop with a warning in the
  log while the party keeps running.

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

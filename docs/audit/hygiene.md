# Audit: Repository hygiene, infrastructure, docs and tests

Findings reference `main` at commit `edd23a33` (2026-09-24), the state before the cleanup branches. Line numbers will drift as the cleanup lands; the file names are stable enough to locate each item.

Severity counts: 1 critical, 10 high, 15 medium, 11 low, 0 info.

## Summary

**Lens: repo hygiene, infra, docs, tests — nsm_rs @ main (edd23a33).**

Repo state verified with `git ls-files`, `git log`, `tar`, `cargo check/clippy/doc/fmt --offline`:

- **Tracked junk**: `nsm-dev-buildx-latest.tar` (18 MB OCI image `sofiamorris/nsm-dev:buildx-latest`, arm64, built 2024-12-12, baked CMD uses the removed `--operation` flag), `src/.DS_Store`, `.env` (another developer's absolute cert paths; nothing in the code reads a .env file), `src/test_event_monitor.sh` (targets non-existent `./target/debug/nsm`, `-o`/`--operation` syntax, `en0`, `tee /dev/tty`, `kill -9`), `view-events-rolebinding.yaml` (k8s RBAC for namespace `nsm-dev` whose paired Role manifest is gitignored).
- **Bloat**: `vendor/` 356 MB tracked (105 MB winapi, ~38 MB windows-sys x2, 55 MB aws-lc-sys, an Android-only crate for an optional dep never enabled); `docs/` 156 MB tracked = full-dependency rustdoc (49 crate dirs) generated 2024-07-23 for the old single `nsm` binary (`docs/src/nsm/main.rs.html`, modules cli/connection/network/service/utils), served via GitHub Pages from `/docs` and linked from the 3-line README. Git pack is 330 MB: history contains `target 2/`, `target 3/` (33 MB debug binaries + dep-graph.bin), `heartbeat_monitor/target`, `test_program/`.
- **Security**: a PEM `-----BEGIN PRIVATE KEY-----` (`server.key`, 28 lines) plus `server.csr` were committed to main in 01a90972 (2025-01-03) and only deleted in ca85e236; they remain in history on `origin/main`, `origin/jpb/sync`, `origin/jpb/sync1`. `.gitignore` never protected them because of the `server.keyl` typo.
- **.gitignore**: only `/target` plus 8 deployment-specific names; two typos (`view-events-rolebinding.yam`, `server.keyl`); no `.env`, `.DS_Store`, `*.pem/*.key/*.csr`, `*.tar`.
- **Docker/compose**: Dockerfile is the unmodified Docker template (feedback-form link, Postgres example in compose); `CMD` uses `--operation` (clap rejects: OPERATION is positional, cli.rs:17-31), `-n en0` (macOS interface name, absent in Linux containers), `--tls` with no CERT_PATH/KEY_PATH provided (tls.rs:22/34 `expect` → panic at startup); builds `api` bin, `EXPOSE 12000` while the REST server is hard-coded to `0.0.0.0:8080` (api.rs:115); rust 1.83.0 pinned vs local 1.98.1; `alpine:3.18` runtime is EOL; `.cargo/config.toml` not mounted so the vendored tree is ignored inside Docker. Non-root user is correct.
- **Cargo.toml**: no `license`/`description`/`repository`/`rust-version`, no LICENSE file on a public GitHub repo; `threadpool` unused (only in comments); `rustls-platform-verifier` optional and never enabled; `[features] ring = []`/`aws-lc-rs = []` are empty — `cargo check --features ring` fails with E0433 (tls.rs:82) because the feature does not forward `rustls/ring`; `aws-lc-rs` only works because hyper-rustls defaults pull it in. `version = "0.1.0"` vs hard-coded `.version("1.0")` in cli.rs:13. Cargo.lock has duplicate versions (rustls-native-certs 0.7.3 + 0.8.1, windows-sys 0.52 + 0.59).
- **CI/quality gates**: no `.github/`, no CI at all, no rust-toolchain.toml/rustfmt.toml/deny.toml. Current baselines: 0 rustc warnings, **158 clippy warnings**, **719 rustfmt diffs**, **6 rustdoc warnings** (unescaped `<IP>:<Port>` in connection.rs:87), 53 TODOs, 8 stale remote branches.
- **Tests**: zero `#[test]`/`#[cfg(test)]`, no `tests/` dir. Structural blockers: no `lib.rs` (modules re-declared in both bins, so `tests/` cannot link anything); `lazy_static` globals `GLOBAL_LAST_HEARTBEAT` (operations.rs:53) and `GLOBAL_MSGBODY` (service.rs:68); `std::process::exit(0)` in library-level code (service.rs:1171-1229, operations.rs:576, mode_api/operations.rs:276); `env::var(..).expect` for TLS paths; ~20 hard-coded `Duration`s; `tcp_server` binds with `unwrap()` and loops forever with no shutdown/`local_addr()`; `stream_read` 1024-byte framing edge (connection.rs:225) and `from_utf8().unwrap()` (connection.rs:222). Positive: all time APIs are `tokio::time` (no `std::time::Instant`/`std::thread` found), so `tokio::time::pause()` is viable once a `Timing` config is injected.
- **Docs**: README is 3 lines and links to stale rendered docs; no ARCHITECTURE/PROTOCOL/CONTRIBUTING/CHANGELOG; `NSM_LOG_LEVEL`, `CERT_PATH`, `KEY_PATH`, `ROOT_PATH` env vars documented nowhere; `missing_docs` cannot be enforced on a bin-only crate.

## Detail

## 1. Exact hygiene change list (branch: `chore/repo-hygiene`, do FIRST)

Working tree:
```
git rm --cached nsm-dev-buildx-latest.tar src/.DS_Store .env
git rm -r --cached docs                     # replaced by CI-built Pages
git rm -r --cached vendor                   # decision A (recommended); see CI section for offline bundle
git rm view-events-rolebinding.yaml         # or: mkdir -p deploy/k8s && git mv ... deploy/k8s/ and add role + README
git mv src/test_event_monitor.sh scripts/smoke_test.sh   # then rewrite for `nsm listen ...` syntax, or delete after integration tests land
git rm README.Docker.md                     # fold into README "Deployment"
```
New `.gitignore`:
```
target*/
**/.DS_Store
.env
.env.*
!.env.example
*.pem
*.key
*.csr
*.p12
*.tar
*.tar.*
*.log
/vendor
/docs
.idea/
.vscode/
```
Add: `.env.example` (CERT_PATH, KEY_PATH, ROOT_PATH, NSM_LOG_LEVEL=info, NSM_LOG_STYLE=auto), `LICENSE`, `rust-toolchain.toml` (`channel="1.98"`, components `rustfmt`,`clippy`), `rustfmt.toml` (`edition="2021"`, `max_width=100`), `clippy.toml` (optional `msrv`), `deny.toml`, `.editorconfig`, `CHANGELOG.md` (Keep-a-Changelog), `CONTRIBUTING.md`, `.github/{workflows,dependabot.yml,CODEOWNERS,pull_request_template.md}`.

Cargo.toml: add `license`, `description`, `repository`, `readme`, `rust-version`, `keywords`; remove `threadpool`, `rustls-platform-verifier`; fix features (`ring = ["rustls/ring","tokio-rustls/ring","hyper-rustls/ring"]`, `aws-lc-rs = [...]`, `default=["aws-lc-rs"]`, `default-features=false` on hyper-rustls/tokio-rustls); `[lib] path="src/lib.rs"` + single `[[bin]] name="nsm" path="src/main.rs"`; `[dev-dependencies]` listed in section 3. Remove `.cargo/config.toml` from the tree (keep as `.cargo/config.offline.toml` template in the vendor bundle).

History rewrite (once, coordinated, after the above lands): `git filter-repo --invert-paths --path 'target 2' --path 'target 3' --path heartbeat_monitor/target --path heartbeat_monitor/vendor --path server.key --path server.csr --path nsm-dev-buildx-latest.tar --path docs --path vendor`; force-push all branches/tags; delete stale remotes (condvar, jpb/sync, jpb/sync1, merge, rest_api, sofia_nsm_rs, jpb/code_cleanup after checking `git log main..`); collaborators re-clone. Rotate the TLS key/cert regardless. Expected pack: 330 MB → single-digit MB.

Baseline formatting: separate commits `cargo fmt --all` (719 hunks) and `cargo clippy --fix --allow-dirty` + manual fixes (158 warnings), then remove the 13 `#[allow(unused*)]` attributes.

## 2. CI design (GitHub Actions)

**Dependency-source decision**: (A, recommended) untrack `vendor/`, delete `.cargo/config.toml`; CI uses crates.io with `--locked` and `Swatinem/rust-cache@v2`. Offline/air-gapped HPC builds are served by a release job that runs `cargo vendor --locked --versioned-dirs vendor` (or `cargo vendor-filterer --platform=x86_64-unknown-linux-gnu --platform=aarch64-unknown-linux-gnu --platform=x86_64-unknown-linux-musl --platform=aarch64-unknown-linux-musl --tier=2`) and uploads `nsm-<tag>-vendor.tar.zst` containing `vendor/` + `.cargo/config.toml`; README documents `tar xf ... && cargo build --release --offline --locked`. (B) if vendor stays tracked: add a job `cargo vendor --locked > /dev/null && git diff --exit-code vendor` to prove vendor matches Cargo.lock, and run all builds with `--offline`.

`.github/workflows/ci.yml` (on push to main + PRs; `concurrency` cancel-in-progress):
- `fmt`: `dtolnay/rust-toolchain@stable` (components rustfmt) → `cargo fmt --all --check`.
- `clippy`: components clippy → `cargo clippy --all-targets --all-features --locked -- -D warnings`; matrix over `--no-default-features --features ring` and default `aws-lc-rs`.
- `test`: matrix `os: [ubuntu-latest, macos-latest]`, `rust: [stable, 1.98 (MSRV from rust-version)]` → `cargo test --all-targets --locked` then `cargo test --doc`; env `RUST_BACKTRACE=1`, `NSM_LOG_LEVEL=debug`; integration tests use 127.0.0.1 ephemeral ports so no privileges needed. Optional `cargo llvm-cov --lcov --output-path lcov.info` + upload to Codecov with a fail-under threshold (start at 60%, ratchet).
- `docs`: `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items`.
- `deny`: `EmbarkStudios/cargo-deny-action@v2` with `deny.toml`: advisories (deny unmaintained/yanked), bans (`multiple-versions="warn"`, deny `openssl`), licenses allowlist (MIT, Apache-2.0, BSD-*, ISC, Unicode-3.0, OpenSSL for aws-lc-sys), sources (crates.io only). Separately `rustsec/audit-check@v2` on a weekly `schedule` so new advisories open issues.
- `unused-deps`: `cargo machete` (fast, no nightly).
- `docker`: `docker/setup-buildx-action` → `docker build --target final .` and `docker run --rm nsm:ci --help` (smoke-tests CLI/CMD consistency; no push on PRs).
- `secrets`: `gitleaks/gitleaks-action@v2`.

`.github/workflows/pages.yml` (on push to main, `permissions: pages: write, id-token: write`): `cargo doc --no-deps`; `<omitted>`; `actions/upload-pages-artifact@v3 with path: target/doc`; `actions/deploy-pages@v4`. Repo Settings → Pages → Source: GitHub Actions. Optionally publish `mdbook` from `docs-src/` (ARCHITECTURE.md, PROTOCOL.md) alongside under `/book`.

`.github/workflows/release.yml` (on `push: tags: ['v*']`): matrix `target: [x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu, x86_64-unknown-linux-musl, aarch64-unknown-linux-musl]`; build with `cargo-zigbuild` (`taiki-e/install-action@cargo-zigbuild`) or `cross`; musl targets use `--no-default-features --features ring` (pure Rust, no cmake, fully static: `RUSTFLAGS=-C target-feature=+crt-static`); `strip`; package `nsm-<tag>-<target>.tar.gz` + `SHA256SUMS`; `softprops/action-gh-release` with `generate_release_notes`. Add `vendor-bundle` job (above) and `docker-publish` job: `docker/metadata-action` + `docker/build-push-action` with `platforms: linux/amd64,linux/arm64` to `ghcr.io/jblaschke/nsm:<tag>,latest`, `provenance: true`. HPC note in README: musl static binaries run on any glibc version (Cray SLES, RHEL 8/9 compute images) without module loads; gnu builds are for perf-sensitive nodes where static PIE is undesirable.

`.github/dependabot.yml`: ecosystems `cargo` (weekly, grouped minor/patch), `github-actions` (weekly), `docker` (weekly).

**Dockerfile fix (single binary, static musl)**:
```dockerfile
# syntax=docker/dockerfile:1
ARG RUST_VERSION=1.98
FROM rust:${RUST_VERSION}-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release --locked --bin nsm --no-default-features --features ring \
 && cp target/release/nsm /nsm

FROM alpine:3.22            # or gcr.io/distroless/static-debian12:nonroot
RUN adduser -D -H -u 10001 -s /sbin/nologin appuser && apk add --no-cache ca-certificates
USER appuser
COPY --from=build /nsm /usr/local/bin/nsm
EXPOSE 12000
HEALTHCHECK CMD ["/usr/local/bin/nsm", "--version"]
ENTRYPOINT ["/usr/local/bin/nsm"]
CMD ["listen", "--bind-port", "12000", "--ip-version", "4"]
```
compose.yaml: `broker` service with `environment: [NSM_LOG_LEVEL=info, CERT_PATH=/certs/cert.pem, KEY_PATH=/certs/key.pem]`, `volumes: [./certs:/certs:ro]`, `command: ["listen","--bind-port","12000","--tls"]` as a commented TLS variant; add `publisher`/`claimer` demo services with `depends_on`. Add `vendor/`, `docs/`, `*.tar`, `target*/` to `.dockerignore`.

## 3. Test strategy (target: `src/lib.rs` + `src/main.rs`)

Prerequisite refactors (each unblocks tests): lib crate; `Timing` config struct; remove globals `GLOBAL_LAST_HEARTBEAT`/`GLOBAL_MSGBODY` into a `Context`; `TlsPaths`/preloaded `rustls` configs instead of env reads; `bind()`/`serve(listener, shutdown)` split with `local_addr()`; `Result` instead of `process::exit`/`panic!`; explicit message framing; `deserialize_message -> Result`.

`[dev-dependencies]`: `tokio = { features = ["test-util","macros","rt","io-util"] }`, `tokio-util` (CancellationToken; also `codec` if adopted as a normal dep), `assert_cmd`, `predicates`, `rcgen` (CA + leaf certs in tests), `tempfile`, `proptest` (Addr/Message round-trips), `pretty_assertions`, `test-log` or `env_logger` (log capture), `reqwest` optional (REST client assertions; hyper-util legacy client already present can be reused).

**Unit tests (in-module `#[cfg(test)]`)**
- `connection::Addr::from_str` / `Display` round trip: `10.0.0.1:8080`, `host:12000`, `http://host:80`, `https://host:443/` (trailing slash), IPv6 `::1:8080` → host `::1`, `[::1]:8080` (currently host `[::1]`; decide and test `to_socket_tuple` semantics — `("[::1]", 8080)` fails `ToSocketAddrs`), `fe80::1%en0:8080`; errors: no port, non-numeric port, `http://a/b:80`, empty string; `to_socket_tuple` rejects `-1`, `70000`, HTTP transport. Property test: `Addr::from_str(&a.to_string()) == Ok(a)`.
- `MessageHeader`/`Message` serde round-trip for every variant; malformed JSON → `Err` (not panic); unknown header → `Err`.
- Framing over `tokio::io::duplex(64)`: single message; message of exactly 1024 bytes and 2048 bytes (current heuristic hangs until timeout); >1024 multi-chunk; multibyte UTF-8 straddling a 1024 boundary; two back-to-back messages; peer closes mid-message; read timeout honoured with `Timing::fast()`.
- `service::FailCounter`: `#[tokio::test(start_paused = true)]` — two `increment()` calls within `interval` count once; `advance(interval)` then increments; reset after `reset_window`; threshold reached exactly at `fail_threshold`.
- `service::State`: `add` assigns monotonically increasing `seq`, groups by key; `claim(key)` returns a service with that key and marks it claimed; `claim(missing)` → `Err(key)`; `rmv` removes only the (key,id) pair; re-claim: remove claimed service → client becomes eligible → next `claim` binds it to the surviving service with the same key; `print` output snapshot.
- `event_monitor` decision logic: extract a pure `fn next_action(hb: &Heartbeat, now: Instant, t: &Timing) -> Action { Keep | RetryLater | Remove | ReclaimClient }` and table-test it with fabricated `Instant`s (paused clock).
- `network`: make `get_interfaces` accept `impl Iterator<Item = InterfaceInfo>`; test name filter, `starting_octets` prefix filter, v4/v6 selection, `only_or_error` behaviour → return `Err(Ambiguous)` instead of panic.
- `cli`: expose `fn command() -> clap::Command`; `command().try_get_matches_from([...])` for each operation; missing `--bind-port` on `listen` → clap error (replace `assert!` with `.requires_if`/`ArgGroup`); `--ip-version 5` → error not panic; `debug_assert()` via `command().debug_assert()`.

**Integration tests (`tests/`)** with `tests/common/mod.rs` helpers: `spawn_broker(transport, timing) -> (SocketAddr, CancellationToken, JoinHandle)`, `spawn_publisher(broker, key, service_port, ping)`, `spawn_claimer(broker, key)`, all binding `127.0.0.1:0` and returning `local_addr()`; every test wrapped in `tokio::time::timeout(30s)`; `Timing::fast()` (hb 50 ms, fail interval 20 ms, threshold 3, read timeout 500 ms) with real time (sockets cannot use paused time).
- `broker_tcp.rs`, `broker_http.rs`, `broker_https.rs` (parametrised over `Transport`; HTTPS uses `rcgen` to mint a CA + `127.0.0.1` SAN leaf into `tempdir`, passed via `TlsPaths`; also a negative test: claimer with wrong root CA fails handshake).
- Scenarios: publish then claim → claimer gets ACK containing publisher Payload (service_addr/port/key); two publishers same key, two claimers → each claimer bound to a distinct service; heartbeats sustained for N intervals with `fail_count == 0`; ping mode (`ping=true`) one-sided liveness; kill publisher task → after `threshold` intervals broker removes it and re-claims its client to the second publisher (assert via a `State` inspection endpoint or an event channel `broker.events()`); `send` → MSG relayed and stored → next HB delivers → `collect` on publisher returns it; broker shutdown → clients return `Outcome::BrokerLost` (no `process::exit`); REST: `GET /list_interfaces`, `POST /publish`, `GET /claim`, `GET /collect`, `POST /send`, unknown route → 404, malformed JSON → 400 (currently `collect_request` unwraps).
- Concurrency stress (`#[ignore]` by default): 50 publishers/50 claimers on one broker.

**CLI tests (`tests/cli.rs`, assert_cmd)**: `nsm --version` == `CARGO_PKG_VERSION`; `nsm --help` lists operations; `nsm list_interfaces` exit 0; `nsm listen` (no `--bind-port`) → exit 2, stderr contains 'required', no backtrace; `nsm publish not-an-addr ...` → clean error; `nsm listen --bind-port 0 --print-addr` prints bound address then is killed (`Command::spawn` + `kill`).

**Making heartbeat timing testable**: `Timing` injected everywhere (`Default` = prod: 5 s fail interval, 60 s window, 10 threshold, 6 s read timeout, 3 s monitor timeout); all sleeps/timeouts via `tokio::time` (already true); logic tests under `#[tokio::test(start_paused = true)]` (current_thread) using `tokio::time::advance`; avoid `spawn_blocking`/`block_in_place` in library code so paused time stays coherent; runtime built in `main` via `Builder` not in the lib. Coverage: `cargo llvm-cov` in CI; target 70% lines on `connection`, `service`, `cli`.

## 4. Docs plan

- **README.md**: What/why (broker for HPC where compute nodes cannot accept inbound connections; services publish, jobs claim); architecture diagram (Mermaid: broker ↔ publisher bind_port, broker ↔ claimer bind_port, claimer → service_port data path; TCP vs HTTP(S) transports); Quickstart (build, `nsm listen --bind-port 12000`, `nsm publish <broker> --bind-port ... --service-port ... --key ...`, `nsm claim ...`, `nsm send`, `nsm collect`); CLI reference (auto-generated via `clap_mangen`/`clap-markdown` in a `xtask` or CI check that the committed table matches); REST API reference (routes from api.rs:139-158 with request/response JSON, status codes, curl examples); TLS setup (generate CA/leaf with openssl or `rcgen` example, `CERT_PATH`/`KEY_PATH`/`ROOT_PATH`, `--root_ca`, provider features `ring`/`aws-lc-rs`); Logging (`NSM_LOG_LEVEL`, `NSM_LOG_STYLE`); Deployment (Docker/compose, ghcr image, k8s manifests in `deploy/k8s`, systemd unit example); HPC notes (static musl binary, Slurm batch example with `srun`, firewall/port ranges, IPv4/IPv6 interface selection with `-n`/`--ip-start`, NERSC Perlmutter specifics); Offline build from the vendor bundle; Contributing/License badges (CI, docs, license).
- **ARCHITECTURE.md**: crate layout after refactor (`cli`, `config`, `transport::{tcp,http}`, `protocol`, `broker::{state,monitor}`, `party::{publisher,claimer}`, `tls`), runtime model (tokio tasks per connection, event_monitor loop, Timing), the transport abstraction trait shared by TCP and REST, error handling policy (no panics/exit below main), configuration precedence (CLI > env > defaults).
- **PROTOCOL.md**: message envelope `{header, body}`; headers HB/ACK/PUB/CLAIM/COL/MSG/NULL with body schemas (Payload fields service_addr, service_port, bind_port, key, root_ca, ping); framing rules; sequence diagrams for publish, claim, two-sided heartbeat, one-sided ping, send/collect relay, failure detection (FailCounter interval/threshold/reset window) and re-claim; REST mapping table (`/request_handler`, `/heartbeat_handler`, `/publish`, `/claim`, `/collect`, `/send`, `/list_*`); versioning/compatibility statement; security model (TLS, key = shared secret, threat notes).
- **CONTRIBUTING.md**: toolchain (rust-toolchain.toml), `cargo fmt`/`clippy -D warnings`/`cargo test`/`cargo deny` before PR, branch naming and PR template, how to run integration tests and the smoke script, how to regenerate CLI docs, commit message style, DCO/CLA note if required by LBNL.
- **CHANGELOG.md**: Keep-a-Changelog format; `Unreleased` section populated during cleanup (breaking: single `nsm` binary, positional OPERATION, removed `--operation`; added transport flag; etc.); tag `v0.2.0` at the end of the cleanup.
- **Rustdoc**: `#![deny(missing_docs)]` and `#![deny(rustdoc::broken_intra_doc_links)]` on `lib.rs`; crate-level `//!` with the README included via `#![doc = include_str!("../README.md")]`; fix `<IP>:<Port>` escapes; doc examples that compile (`cargo test --doc`) for `Addr::from_str`, `Message` serialisation and `Timing`; CI deploys `cargo doc --no-deps` to Pages (replacing the tracked `docs/`); add `docs/` (source, e.g. mdBook with ARCHITECTURE/PROTOCOL) only if kept as authored Markdown, never generated HTML.
- **Deploy docs**: `deploy/k8s/README.md` explaining Role/RoleBinding purpose (event viewing in `nsm-dev`), Deployment/Service manifests, and how TLS secrets are mounted.

## Findings

Ids are `H` plus the finding number, in the order the reviewer reported them (not by severity).

| # | Severity | Category | Finding | Location |
|---|---|---|---|---|
| H1 | critical | security | Private TLS key and CSR committed to main history (still recoverable) | `.gitignore:9` |
| H2 | high | repo-hygiene | 18 MB Docker image tarball tracked in git | `nsm-dev-buildx-latest.tar:1` |
| H3 | high | repo-hygiene | docs/ is 156 MB of stale generated rustdoc for a binary that no longer exists | `docs/index.html:2` |
| H4 | high | repo-hygiene | vendor/ (356 MB) tracked, including ~200 MB of Windows-only and never-enabled crates | `.cargo/config.toml:1` |
| H5 | high | repo-hygiene | Git history bloated with committed build outputs (330 MB pack) | `.gitignore:1` |
| H6 | medium | repo-hygiene | .gitignore typo leaves k8s RoleBinding tracked | `.gitignore:7` |
| H7 | medium | repo-hygiene | .gitignore lacks standard entries (.env, .DS_Store, certs, archives) | `.gitignore:1` |
| H8 | medium | repo-hygiene | Tracked .env contains another developer's absolute cert paths and is never read | `.env:1` |
| H9 | low | repo-hygiene | src/.DS_Store tracked | `src/.DS_Store:1` |
| H10 | medium | tests | Stale smoke-test script uses removed binary name and CLI syntax, lives in src/ | `src/test_event_monitor.sh:5` |
| H11 | high | infra | Dockerfile CMD uses removed `--operation` flag; container exits with a clap error | `Dockerfile:73` |
| H12 | medium | infra | Dockerfile CMD hard-codes macOS interface name `en0` inside a Linux container | `Dockerfile:73` |
| H13 | high | infra | Dockerfile enables --tls but provides no certificate, key, or env; broker panics at startup | `Dockerfile:73` |
| H14 | medium | security | Runtime image alpine:3.18 is end-of-life | `Dockerfile:49` |
| H15 | low | infra | Pinned Rust 1.83.0 in Dockerfile drifts from the 1.98.1 toolchain used locally; no rust-toolchain.toml | `Dockerfile:9` |
| H16 | medium | infra | Dockerfile builds the `api` binary but EXPOSEs 12000 while the REST server is hard-coded to 0.0.0.0:8080 | `Dockerfile:10` |
| H17 | low | infra | Docker build stage ignores the repo's vendored-source config and builds both binaries | `Dockerfile:30` |
| H18 | low | docs | Dockerfile/compose/README.Docker.md are unmodified Docker template boilerplate | `Dockerfile:7` |
| H19 | high | docs | README is three lines and points to stale generated docs | `README.md:3` |
| H20 | medium | repo-hygiene | No LICENSE file and no license/description/repository metadata in Cargo.toml | `Cargo.toml:1` |
| H21 | medium | infra | Empty `ring`/`aws-lc-rs` features: `--features ring` fails to compile | `Cargo.toml:32` |
| H22 | low | dependencies | Unused dependency `threadpool` | `Cargo.toml:15` |
| H23 | low | dependencies | Optional dependency `rustls-platform-verifier` is never enabled by any feature | `Cargo.toml:23` |
| H24 | low | docs | CLI reports hard-coded version "1.0" while the crate is 0.1.0 | `src/cli.rs:13` |
| H25 | low | dependencies | Duplicate crate versions in Cargo.lock | `Cargo.lock:1` |
| H26 | high | infra | No continuous integration; 158 clippy warnings, 719 rustfmt diffs and 6 rustdoc warnings would fail any gate | `Cargo.toml:1` |
| H27 | low | docs | rustdoc warnings: unescaped angle brackets in doc comment | `src/connection.rs:87` |
| H28 | high | tests | Zero tests in the crate | `Cargo.toml:37` |
| H29 | high | tests | No lib.rs: modules are re-declared in both binaries, so integration tests cannot link any code | `src/tcp.rs:8` |
| H30 | medium | tests | Process-wide lazy_static globals block running broker and clients in one test process | `src/operations.rs:53` |
| H31 | medium | tests | `std::process::exit(0)` inside library-level code kills the test harness | `src/service.rs:1171` |
| H32 | medium | tests | TLS and root-CA paths read from environment with `expect`, untestable in-process | `src/tls.rs:22` |
| H33 | medium | tests | ~20 hard-coded heartbeat/timeout durations make timing behaviour untestable and un-tunable | `src/service.rs:93` |
| H34 | medium | tests | tcp_server binds with unwrap and never returns; no ephemeral-port or shutdown support | `src/connection.rs:340` |
| H35 | medium | tests | Raw-TCP message framing relies on a 1024-byte short-read heuristic | `src/connection.rs:225` |
| H36 | low | repo-hygiene | Eight stale remote branches | `.gitignore:1` |
| H37 | low | infra | Broker binary hard-codes a 20-thread tokio runtime; REST binary uses default | `src/tcp.rs:41` |

### H1. Private TLS key and CSR committed to main history (still recoverable)

**Severity:** critical  
**Category:** security  
**Location:** `.gitignore:9`

`server.key` (28-line `-----BEGIN PRIVATE KEY-----` PEM) and `server.csr` were added in commit 01a90972 ("optional root_cert", 2025-01-03) and deleted in ca85e236/92acfe01, but the blobs remain reachable on origin/main, origin/jpb/sync and origin/jpb/sync1 (`git show 01a90972:server.key | head -1` prints the PEM header). .gitignore line 9 reads `server.keyl` (typo) so the key was never ignored; line 8 ignores `server.csr` but it was force-added anyway.

**Fix:** Treat the key as compromised: revoke/rotate the certificate it backs. Rewrite history with `git filter-repo --invert-paths --path server.key --path server.csr` (bundle with the other history-bloat removals), force-push all branches, ask collaborators to re-clone. Fix .gitignore to `*.key`, `*.pem`, `*.csr`, `*.p12`. Add a pre-commit secret scan (gitleaks) to CI.

### H2. 18 MB Docker image tarball tracked in git

**Severity:** high  
**Category:** repo-hygiene  
**Location:** `nsm-dev-buildx-latest.tar:1`

OCI image export of `docker.io/sofiamorris/nsm-dev:buildx-latest` (arm64, created 2024-12-12), added in merge 2d225eb4. Its baked config `Cmd` is `["-n","en0","--ip-version","4","--operation","listen","--bind-port","12000","--tls"]`, which the current CLI rejects (no `--operation` flag), so the image is also non-functional. Binary artifacts do not belong in a source repo and inflate every clone.

**Fix:** `git rm --cached nsm-dev-buildx-latest.tar`, add `*.tar` to .gitignore, purge from history with filter-repo. Publish images to a registry (ghcr.io) from CI instead.

### H3. docs/ is 156 MB of stale generated rustdoc for a binary that no longer exists

**Severity:** high  
**Category:** repo-hygiene  
**Location:** `docs/index.html:2`

`git log -- docs` shows two commits (bc6524ad 'Add rustdocs' 2024-07-23, 4b2b180e 2024-07-24). `docs/index.html` redirects to `nsm/`, and `docs/src/nsm/` contains `main.rs.html`, `cli.rs.html`, `connection.rs.html`, `network.rs.html`, `service.rs.html`, `utils.rs.html` — the pre-REST single-binary layout (no api_builder, tls, models, mode_*). 49 dependency crate directories are included (rustdoc was run without `--no-deps`; 25 MB under docs/src alone). README.md:3 advertises this as the rendered documentation, so users get docs for a CLI (`-o/--operation`) that no longer exists.

**Fix:** `git rm -r --cached docs` and purge from history. Build docs in CI (`RUSTDOCFLAGS='-D warnings' cargo doc --no-deps`) and deploy with `actions/upload-pages-artifact` + `actions/deploy-pages`; switch the repository Pages source from 'Deploy from a branch /docs' to 'GitHub Actions'.

### H4. vendor/ (356 MB) tracked, including ~200 MB of Windows-only and never-enabled crates

**Severity:** high  
**Category:** repo-hygiene  
**Location:** `.cargo/config.toml:1`

`.cargo/config.toml` replaces crates-io with `directory = "vendor"` for every build. `cargo vendor` pulls every platform and every optional dependency: vendor/winapi-x86_64-pc-windows-gnu 54 MB, winapi-i686 51 MB, windows-sys + windows-sys-0.52.0 38 MB, six windows_* target crates ~50 MB, aws-lc-sys 55 MB, rustls-platform-verifier-android (for an optional dep never enabled). Every dependency bump produces a multi-thousand-file diff; the Dockerfile does not mount .cargo/config.toml so Docker builds bypass vendor anyway, making the tree inconsistent with the deploy path.

**Fix:** Preferred: stop tracking vendor/ (`git rm -r --cached vendor`, add `/vendor` to .gitignore), rely on Cargo.lock + `--locked` + CI cache, and produce an offline bundle only at release time (`cargo vendor --locked` or `cargo vendor-filterer --platform=x86_64-unknown-linux-gnu --platform=aarch64-unknown-linux-gnu --platform=*-linux-musl` → `nsm-vendor-<tag>.tar.zst` with a matching `.cargo/config.toml`) for air-gapped HPC builds. If vendoring must stay in-tree, at minimum use cargo-vendor-filterer to drop Windows/Android/wasm crates and add a CI check that `cargo build --offline --locked` succeeds.

### H5. Git history bloated with committed build outputs (330 MB pack)

**Severity:** high  
**Category:** repo-hygiene  
**Location:** `.gitignore:1`

`git count-objects -vH` reports a 329.57 MiB pack. `git rev-list --objects --all` shows the largest blobs are `target 3/debug/api` (33 MB), `target 2/debug/api` (33 MB), `target 2/debug/deps/tcp-*` (31 MB), several 20+ MB `dep-graph.bin` and `.rlib` files, committed in 35e5352a/83ad0b60 (macOS Finder duplicates 'target 2', 'target 3' not covered by `/target`), plus `heartbeat_monitor/target/**`, `heartbeat_monitor/vendor/**`, `test_program/**`, `.cargo/config_novendor.toml`, `client_test.sh`, `listener_test.sh`, `server_test.sh`, `sshproxy.sh`, `test.sh` in older commits.

**Fix:** One coordinated `git filter-repo` pass removing: `target 2`, `target 3`, `heartbeat_monitor/target`, `heartbeat_monitor/vendor`, `server.key`, `server.csr`, `nsm-dev-buildx-latest.tar`, `docs`, `vendor`. Expect the pack to drop from ~330 MB to a few MB. Change `.gitignore` `/target` to `target*/` and add `**/.DS_Store`. Do this once, early in the cleanup, before opening the planned branches.

### H6. .gitignore typo leaves k8s RoleBinding tracked

**Severity:** medium  
**Category:** repo-hygiene  
**Location:** `.gitignore:7`

Line 7 is `view-events-rolebinding.yam` (missing `l`), so `view-events-rolebinding.yaml` is tracked at the repo root while its paired `view-events-role.yaml` (line 6) is ignored. The tracked file references `Role view-events` in namespace `nsm-dev` that does not exist in the repo.

**Fix:** Either delete the binding, or create `deploy/k8s/` holding both Role and RoleBinding (plus a Deployment/Service for the broker) with a short README, and remove the deployment file names from .gitignore.

### H7. .gitignore lacks standard entries (.env, .DS_Store, certs, archives)

**Severity:** medium  
**Category:** repo-hygiene  
**Location:** `.gitignore:1`

Only `/target` plus 8 hand-listed filenames. Missing: `.env`, `**/.DS_Store`, `*.pem`, `*.key`, `*.csr`, `*.tar`, `*.tar.gz`, `/vendor` (if untracked), `/docs` (if untracked), editor dirs. This directly caused `src/.DS_Store`, `.env`, the tarball and the private key to be committed.

**Fix:** Replace with: `target*/`, `**/.DS_Store`, `.env`, `.env.*`, `!.env.example`, `*.pem`, `*.key`, `*.csr`, `*.p12`, `*.tar`, `*.tar.*`, `*.log`, `/vendor`, `/docs`, `.idea/`, `.vscode/`, `*.swp`.

### H8. Tracked .env contains another developer's absolute cert paths and is never read

**Severity:** medium  
**Category:** repo-hygiene  
**Location:** `.env:1`

`CERT_PATH=/Users/sofiamorris/Desktop/certs/listen/cert.pem` / `KEY_PATH=/Users/sofiamorris/Desktop/certs/listen/key.pem`. No `dotenv` crate is in Cargo.toml and `grep -rn dotenv src` is empty, so the file is inert for the binaries; `.dockerignore:10` excludes it from images. It only leaks a contributor's local filesystem layout and misleads new developers into thinking it is loaded.

**Fix:** `git rm --cached .env`; add `.env` to .gitignore; commit a `.env.example` documenting `CERT_PATH`, `KEY_PATH`, `ROOT_PATH`, `NSM_LOG_LEVEL`, `NSM_LOG_STYLE` with placeholder values; document them in README.

### H9. src/.DS_Store tracked

**Severity:** low  
**Category:** repo-hygiene  
**Location:** `src/.DS_Store:1`

macOS Finder metadata file added in 960fbd77 (2024-08-09). `.dockerignore:7` already excludes `**/.DS_Store` but .gitignore does not.

**Fix:** `git rm --cached src/.DS_Store`; add `**/.DS_Store` to .gitignore (and to a global gitignore on developer machines).

### H10. Stale smoke-test script uses removed binary name and CLI syntax, lives in src/

**Severity:** medium  
**Category:** tests  
**Location:** `src/test_event_monitor.sh:5`

Line 5 runs `./target/debug/nsm -n en0 -o list_ips ...` and line 7 `--operation listen`; the crate now builds `tcp` and `api` binaries (Cargo.toml:37-43) and the CLI takes OPERATION positionally with no `-o`/`--operation` (cli.rs:17-31). Also hard-codes the macOS interface `en0`, pipes through `tee /dev/tty` (fails without a TTY, e.g. CI), and uses `kill -9` (line 46). It is the only test-like artifact in the repo and sits inside `src/` where cargo/rustdoc tooling does not expect shell scripts.

**Fix:** Delete it once the in-process integration tests exist (see extra), or move to `scripts/smoke_test.sh`, rewrite against the new single `nsm` binary and positional syntax, auto-detect the interface, drop `tee /dev/tty`, and use `kill -TERM` + `wait` with a trap for cleanup.

### H11. Dockerfile CMD uses removed `--operation` flag; container exits with a clap error

**Severity:** high  
**Category:** infra  
**Location:** `Dockerfile:73`

`CMD ["-n", "en0", "--ip-version", "4", "--operation", "listen", "--bind-port", "12000", "--tls"]`. cli.rs:16-31 defines OPERATION as `.index(1).required(true)`; there is no `--operation` argument, so clap prints 'unexpected argument' and exits 2. The shipped tarball image carries the same broken Cmd.

**Fix:** `CMD ["listen", "--bind-port", "12000", "--ip-version", "4"]` (after the CLI is consolidated into one `nsm` binary). Add a CI job that runs `docker build` and `docker run --rm image --help` so CLI/Dockerfile drift fails the build.

### H12. Dockerfile CMD hard-codes macOS interface name `en0` inside a Linux container

**Severity:** medium  
**Category:** infra  
**Location:** `Dockerfile:73`

`-n en0` is a macOS interface name. Linux containers expose `eth0`/`lo`. network.rs:98 filters interfaces by exact name (`ip.name == *iname`), so no addresses match and `listen` fails (utils.rs:7 `only_or_error` panics on a non-single vector).

**Fix:** Drop `-n` from the default CMD (let the broker bind `0.0.0.0` or the single non-loopback address), or make the interface configurable via env (`NSM_INTERFACE`) documented in compose.yaml.

### H13. Dockerfile enables --tls but provides no certificate, key, or env; broker panics at startup

**Severity:** high  
**Category:** infra  
**Location:** `Dockerfile:73`

CMD passes `--tls`; tls.rs:22 `env::var("CERT_PATH").expect("CERT_PATH not set")` and tls.rs:34 `KEY_PATH` panic if unset. Neither Dockerfile (no `ENV`) nor compose.yaml (no `environment:`/`volumes:`/`secrets:`) supplies them, and `.dockerignore:10` excludes `.env`.

**Fix:** Make TLS opt-in at runtime: default CMD without `--tls`; in compose.yaml add `environment: [CERT_PATH=/certs/cert.pem, KEY_PATH=/certs/key.pem]` and a read-only `volumes: [./certs:/certs:ro]` or Docker `secrets`. Replace `expect` with a proper error so misconfiguration prints a message rather than a backtrace.

### H14. Runtime image alpine:3.18 is end-of-life

**Severity:** medium  
**Category:** security  
**Location:** `Dockerfile:49`

`FROM alpine:3.18 AS final`. Alpine 3.18 reached end of support in May 2025; no further security patches for the base layer (musl, busybox, ca-certificates).

**Fix:** Use a current Alpine (3.22 as of 2025, verify at build time) pinned by digest, or ship a fully static musl binary on `gcr.io/distroless/static` / `scratch`. Add Dependabot `docker` ecosystem to bump base images.

### H15. Pinned Rust 1.83.0 in Dockerfile drifts from the 1.98.1 toolchain used locally; no rust-toolchain.toml

**Severity:** low  
**Category:** infra  
**Location:** `Dockerfile:9`

`ARG RUST_VERSION=1.83.0` (Dec 2024). Local toolchain is rustc 1.98.1. There is no `rust-toolchain.toml` and Cargo.toml has no `rust-version`, so nothing states the supported compiler and the Docker build can silently diverge (e.g. newer dependency versions after a bump may require a newer MSRV).

**Fix:** Add `rust-version` to Cargo.toml and a `rust-toolchain.toml` (`channel = "1.98"`, components rustfmt+clippy); derive the Dockerfile ARG from it or keep both updated by Dependabot/CI MSRV check (`cargo +<msrv> check`).

### H16. Dockerfile builds the `api` binary but EXPOSEs 12000 while the REST server is hard-coded to 0.0.0.0:8080

**Severity:** medium  
**Category:** infra  
**Location:** `Dockerfile:10`

`ARG APP_NAME=api` copies `target/release/api` to `/bin/server`; `EXPOSE 12000` (line 68) and compose maps `12000:12000`. api.rs:115 hard-codes the REST listener to `"0.0.0.0:8080"` with no CLI flag or env override, and api.rs:114 logs the address as '0.0.0.0.1:8080'. Running the container with no operation argument would serve REST on 8080, unreachable via the documented port.

**Fix:** Make the REST bind address a CLI/env option (`--api-bind 0.0.0.0:8080`) and expose both ports (or a single one) consistently in Dockerfile, compose.yaml and README. After consolidation to one `nsm` binary, set `APP_NAME=nsm` and build only that target: `cargo build --release --locked --bin nsm`.

### H17. Docker build stage ignores the repo's vendored-source config and builds both binaries

**Severity:** low  
**Category:** infra  
**Location:** `Dockerfile:30`

Only `src`, `Cargo.toml`, `Cargo.lock` are bind-mounted (lines 30-32); `.cargo/config.toml` is not, so `cargo build --locked --release` downloads from crates.io, contradicting the in-repo vendoring story. `cargo build` without `--bin` compiles both `tcp` and `api` (line 36) though only `$APP_NAME` is copied. The aws-lc-sys C build on musl may additionally need `cmake`/`perl` which are not installed (line 20 installs only clang, lld, musl-dev, git).

**Fix:** Decide on one dependency-source strategy (see vendor finding). Build with `--bin nsm`; for the musl/alpine image prefer `--features ring` (or `rustls/ring`) to avoid the aws-lc-sys cmake toolchain, or add `cmake perl` to the apk line.

### H18. Dockerfile/compose/README.Docker.md are unmodified Docker template boilerplate

**Severity:** low  
**Category:** docs  
**Location:** `Dockerfile:7`

Dockerfile lines 3-7 include the template's 'Want to help us make this template better? Share your feedback here: https://forms.gle/...' link; lines 39-48 are generic multi-stage explanations. compose.yaml lines 1-9 and 18-50 are template comments plus a commented Postgres example irrelevant to a connection broker. README.Docker.md refers to 'myapp' and `docker compose up --build` exposing 'http://localhost:12000' (the TCP broker is not HTTP).

**Fix:** Strip template comments; keep a short header describing the two stages. Rewrite compose.yaml as a real example: a `broker` service (listen), optional `publisher`/`claimer` services on the same network for a demo, cert volume, env. Fold README.Docker.md into README's Deployment section and delete it.

### H19. README is three lines and points to stale generated docs

**Severity:** high  
**Category:** docs  
**Location:** `README.md:3`

Entire README: a title and `Rendered docs: https://jblaschke.github.io/nsm_rs`. The linked site is the 2024 rustdoc of the removed `nsm` binary (see docs/ finding). There is no statement of purpose, protocol overview, build instructions, CLI usage, REST API description, TLS setup (CERT_PATH/KEY_PATH/ROOT_PATH), logging (`NSM_LOG_LEVEL`, `NSM_LOG_STYLE` at tcp.rs:46-47), or deployment notes.

**Fix:** Write the README per the docs plan in `extra` (what/why, architecture diagram, quickstart, CLI reference, REST reference, TLS, deployment, HPC notes). Point the docs link at the CI-built Pages site.

### H20. No LICENSE file and no license/description/repository metadata in Cargo.toml

**Severity:** medium  
**Category:** repo-hygiene  
**Location:** `Cargo.toml:1`

`[package]` has only name, version, edition. `ls LICENSE*` finds nothing. The repository is public at github.com/JBlaschke/nsm_rs; without a license, contributions and reuse (including by NERSC users) are legally ambiguous. `.dockerignore:31` even excludes a `LICENSE` that does not exist.

**Fix:** Add `LICENSE` (owner's choice; LBNL/NERSC projects commonly use a BSD-3-Clause with LBNL notice — confirm with the lab's licensing process) and Cargo.toml fields: `license`, `description`, `repository`, `readme`, `keywords`, `categories`, `rust-version`, `authors`.

### H21. Empty `ring`/`aws-lc-rs` features: `--features ring` fails to compile

**Severity:** medium  
**Category:** infra  
**Location:** `Cargo.toml:32`

`[features] ring = []` and `aws-lc-rs = []` forward nothing. tls.rs:81-84 gates `rustls::crypto::ring::default_provider()` / `aws_lc_rs` on them, and `rustls` is declared with `default-features = false` (line 25). `cargo check --offline --features ring` fails: `error[E0433]: cannot find 'ring' in 'crypto'` (3x). `--features aws-lc-rs` only compiles because hyper-rustls 0.27 default features pull `rustls/aws-lc-rs` transitively. With neither feature, no provider is installed explicitly and the build relies on the transitive default.

**Fix:** `ring = ["rustls/ring", "tokio-rustls/ring", "hyper-rustls/ring"]`, `aws-lc-rs = ["rustls/aws-lc-rs", "tokio-rustls/aws-lc-rs", "hyper-rustls/aws-lc-rs"]`, `default = ["aws-lc-rs"]`, and disable hyper-rustls/tokio-rustls default features so exactly one provider is compiled. CI should build both feature sets (musl release with `ring`).

### H22. Unused dependency `threadpool`

**Severity:** low  
**Category:** dependencies  
**Location:** `Cargo.toml:15`

`threadpool = "1.8"` is declared; `grep -rn threadpool src` matches only comments (service.rs:396, service.rs:484). It is vendored and compiled for nothing; tokio already provides the runtime.

**Fix:** Remove the dependency and the two stale comments. Add `cargo machete` or `cargo udeps` to CI to catch unused deps.

### H23. Optional dependency `rustls-platform-verifier` is never enabled by any feature

**Severity:** low  
**Category:** dependencies  
**Location:** `Cargo.toml:23`

`rustls-platform-verifier = { version = "0.3", optional = true }` with no feature referencing it and no `use` in src. It still lands in Cargo.lock and vendor/ (including `rustls-platform-verifier-android`).

**Fix:** Remove it, or add a real `platform-verifier` feature and use it in tls.rs `load_ca` as an alternative to `rustls-native-certs`.

### H24. CLI reports hard-coded version "1.0" while the crate is 0.1.0

**Severity:** low  
**Category:** docs  
**Location:** `src/cli.rs:13`

`Command::new("NERSC Service Mesh").version("1.0")` vs Cargo.toml:3 `version = "0.1.0"`. `nsm --version` will lie once releases are cut.

**Fix:** Use `.version(env!("CARGO_PKG_VERSION"))` (or clap's `crate_version!`) and set the binary name to `nsm`.

### H25. Duplicate crate versions in Cargo.lock

**Severity:** low  
**Category:** dependencies  
**Location:** `Cargo.lock:1`

Cargo.lock contains `rustls-native-certs` 0.7.3 and 0.8.1 (0.7 pulled by hyper-rustls 0.27.5, 0.8 declared directly) and `windows-sys` 0.52.0 and 0.59.0. Each duplicate is compiled twice and vendored twice (vendor/windows-sys and vendor/windows-sys-0.52.0).

**Fix:** Add `deny.toml` with `[bans] multiple-versions = "warn"` and run `cargo deny check` in CI; after the dependency bump, align versions (hyper-rustls 0.27.x newer releases use rustls-native-certs 0.8).

### H26. No continuous integration; 158 clippy warnings, 719 rustfmt diffs and 6 rustdoc warnings would fail any gate

**Severity:** high  
**Category:** infra  
**Location:** `Cargo.toml:1`

No `.github/workflows`, `.gitlab-ci.yml`, Makefile or justfile exists. Measured baselines: `cargo clippy --offline` → 158 warnings (40 redundant field names, 17 needless return, 15 let-unit, 14 map_or, 10 needless borrow, ...); `cargo fmt --check` → 719 `Diff in` hunks; `cargo doc --no-deps` → 6 warnings; `cargo check` → 0 rustc warnings (only because of 13 `#[allow(unused_imports)]`/`#[allow(unused)]` attributes). Nothing prevents regressions.

**Fix:** Add the CI design in `extra`: fmt check, `clippy --all-targets --all-features -- -D warnings`, tests, `RUSTDOCFLAGS=-D warnings cargo doc`, cargo-deny/audit, Pages deploy, release matrix, Docker build. Land `cargo fmt` and `cargo clippy --fix` as their own commits before enabling `-D warnings`.

### H27. rustdoc warnings: unescaped angle brackets in doc comment

**Severity:** low  
**Category:** docs  
**Location:** `src/connection.rs:87`

`/// <IP>:<Port> or http://<Name>:<Port> or https://<Name>:<Port>` produces 6 'unclosed HTML tag' warnings (`cargo doc --no-deps`). Additionally connection.rs:1 uses `///` (outer doc) on the first `use` statement instead of `//!` module docs, and clippy reports 5 'empty line after doc comment' cases. With `-D warnings` on rustdoc, docs deployment would fail.

**Fix:** Wrap in backticks: `` `<IP>:<Port>` ``; change file-header comments to `//!`; remove blank lines between doc comments and items.

### H28. Zero tests in the crate

**Severity:** high  
**Category:** tests  
**Location:** `Cargo.toml:37`

`grep -rn '#[test]|#[cfg(test)]|#[tokio::test]' src` → 0; no `tests/` directory; no `[dev-dependencies]`. Protocol behaviour (PUB/CLAIM/HB/COL/MSG, FailCounter threshold 10 at service.rs:335/594, re-claim on service death) is entirely unverified, and the pending TCP/REST deduplication has no safety net.

**Fix:** Adopt the test strategy in `extra`: lib crate + unit tests for Addr/Message/State/FailCounter, in-process integration tests over TCP/HTTP/HTTPS on ephemeral ports, assert_cmd CLI tests, `cargo llvm-cov` in CI.

### H29. No lib.rs: modules are re-declared in both binaries, so integration tests cannot link any code

**Severity:** high  
**Category:** tests  
**Location:** `src/tcp.rs:8`

tcp.rs:8-23 and api.rs:8-25 each `mod service; mod models; mod tls; mod network; mod mode_api; mod mode_tcp; mod connection; mod utils; mod cli; mod operations;`. Cargo.toml has only two `[[bin]]` targets (37-43). Cargo `tests/*.rs` can only depend on a library target, and `missing_docs`/rustdoc coverage only applies to public items of a library. Every module is compiled twice.

**Fix:** Add `src/lib.rs` (`pub mod cli; pub mod connection; ...`) with `#![deny(missing_docs)]`, one `[[bin]] name = "nsm" path = "src/main.rs"` that is a thin `use nsm::...` wrapper, and select transport at runtime (`--transport tcp|http|https` or subcommands).

### H30. Process-wide lazy_static globals block running broker and clients in one test process

**Severity:** medium  
**Category:** tests  
**Location:** `src/operations.rs:53`

`GLOBAL_LAST_HEARTBEAT: Arc<Mutex<Option<Instant>>>` (operations.rs:53-55, written from connection.rs:316-318) and `GLOBAL_MSGBODY: Mutex<MsgBody>` (service.rs:68-70) are singletons. An in-process integration test that spawns a broker, a publisher and a claimer would have all three share (and clobber) the same heartbeat timestamp and message body.

**Fix:** Move both into a per-party context struct (`Broker`/`Party` state or `Arc<Context>`) passed to handlers; the hyper router in connection.rs:299-327 should receive it via closure capture. This is a prerequisite for the integration tests in `extra`.

### H31. `std::process::exit(0)` inside library-level code kills the test harness

**Severity:** medium  
**Category:** tests  
**Location:** `src/service.rs:1171`

`std::process::exit(0)` appears at service.rs:1171, 1179, 1217, 1221, 1225, 1229 (all tagged `// TODO: Don't exist proc insitu`), operations.rs:576 and mode_api/operations.rs:276. Any test that exercises a heartbeat timeout path will terminate the whole `cargo test` process with exit code 0 (falsely green). Numerous `panic!` sites (service.rs:258, 738, 745-750, 990, 1234; operations.rs:509-522, 686, 765-773) have the same effect on async tasks.

**Fix:** Return `Result<Outcome, Error>` (e.g. `Outcome::BrokerLost`) up to `main`, which alone decides the exit code. Replace `panic!` on protocol violations with typed errors.

### H32. TLS and root-CA paths read from environment with `expect`, untestable in-process

**Severity:** medium  
**Category:** tests  
**Location:** `src/tls.rs:22`

tls.rs:22 `env::var("CERT_PATH").expect(...)`, tls.rs:34 `KEY_PATH`, tls.rs:149 and operations.rs:438/697/822 `ROOT_PATH`. Environment variables are process-global; two TLS parties in one test process cannot have different certs, and `std::env::set_var` is unsafe in Rust 2024 edition. Errors are panics, not `Result`s.

**Fix:** Introduce `pub struct TlsPaths { cert: PathBuf, key: PathBuf, root_ca: Option<PathBuf> }` (or already-loaded `ServerConfig`/`RootCertStore`) resolved from CLI flags with env fallback in `main` only, and pass it down. Tests generate certs with `rcgen` into a `tempfile::tempdir()`.

### H33. ~20 hard-coded heartbeat/timeout durations make timing behaviour untestable and un-tunable

**Severity:** medium  
**Category:** tests  
**Location:** `src/service.rs:93`

FailCounter `interval: Duration::from_secs(5)` (service.rs:93); monitor timeout 3 s (232); 60 s windows (279, 574); threshold `== 10` (335, 594); sleeps 200 ms (316, 480), 300 ms (839, 922), 702 ms (984), 1 s (532, 1120, 1308), 2 s (1062), 10 s (1082, 1184, 1241); connection.rs:215 read timeout 6 s; operations.rs:341/726/850 6 s request timeout, 563-565 5 s/500 ms, 574 10 s; mode_api/operations.rs:184-274 duplicates the same constants. A realistic failure-detection test (10 failures x 5 s) would take ~60 s wall-clock per case.

**Fix:** Add `pub struct Timing { hb_interval, hb_timeout, fail_increment_interval, fail_threshold: u32, reset_window, read_timeout, request_timeout }` with `Default` = current values and `Timing::fast()` for tests; thread it through `State`, `Heartbeat`, `event_monitor` and the client loops. All timers already use `tokio::time` (no `std::time::Instant`/`std::thread::sleep` in src), so `#[tokio::test(start_paused = true)]` + `tokio::time::advance` works for logic tests.

### H34. tcp_server binds with unwrap and never returns; no ephemeral-port or shutdown support

**Severity:** medium  
**Category:** tests  
**Location:** `src/connection.rs:340`

`TcpListener::bind(format!("{}:{}", addr.host, addr.port)).await.unwrap()` (connection.rs:340-342) then `loop { accept }` with an unreachable `Ok(())` (359). Tests cannot ask for port 0 and learn the bound port, nor stop the broker cleanly; handler futures are awaited inline (351) so one slow client blocks accept.

**Fix:** Split into `bind(addr) -> io::Result<TcpListener>` (callers read `local_addr()`) and `serve(listener, handler, shutdown: CancellationToken)` using `tokio::select!`; spawn each connection handler. Same for the hyper server in api.rs:116-127.

### H35. Raw-TCP message framing relies on a 1024-byte short-read heuristic

**Severity:** medium  
**Category:** tests  
**Location:** `src/connection.rs:225`

`stream_read` loops while `bytes_read == buf.len()` (connection.rs:213-228). A JSON message of exactly 1024 bytes (or any multiple) makes the reader issue another `read` that blocks until the 6 s timeout (line 215) and returns `TimedOut`; a 1024-byte TCP segment boundary splitting a multi-byte UTF-8 char panics at `from_utf8(...).unwrap()` (line 222); `deserialize_message` (194-196) `unwrap`s on any malformed payload. These are exactly the cases unit tests should pin down.

**Fix:** Adopt explicit framing (newline-delimited JSON via `tokio_util::codec::LinesCodec`, or `LengthDelimitedCodec` + `serde_json`), return `Result` from deserialize, and add `tokio::io::duplex`-based round-trip tests including 1024-byte, >1024-byte, split-UTF-8 and garbage inputs.

### H36. Eight stale remote branches

**Severity:** low  
**Category:** repo-hygiene  
**Location:** `.gitignore:1`

`git branch -a` lists origin/condvar, origin/jpb/code_cleanup, origin/jpb/sync, origin/jpb/sync1, origin/merge, origin/rest_api, origin/sofia_nsm_rs alongside main. Several carry the leaked key in history; all predate the current cleanup and confuse the planned per-action branch workflow.

**Fix:** For each branch: `git log main..origin/<b>` to confirm it is merged or obsolete, then delete on the remote (before/after the filter-repo rewrite, which must include them). Enable branch protection on main requiring CI.

### H37. Broker binary hard-codes a 20-thread tokio runtime; REST binary uses default

**Severity:** low  
**Category:** infra  
**Location:** `src/tcp.rs:41`

`#[tokio::main(flavor = "multi_thread", worker_threads = 20)]` (tcp.rs:41) vs plain `#[tokio::main]` (api.rs:53). On HPC login/compute nodes with cgroup CPU limits or many co-located brokers, a fixed 20 OS threads is wasteful; the inconsistency also disappears only if the binaries are merged.

**Fix:** Use the default (num CPUs) or an env/CLI override (`NSM_WORKER_THREADS`) in the single `nsm` main; build the runtime with `tokio::runtime::Builder` so tests can use `current_thread`.


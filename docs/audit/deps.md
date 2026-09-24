# Audit: Dependencies

Findings reference `main` at commit `edd23a33` (2026-09-24), the state before the cleanup branches. Line numbers will drift as the cleanup lands; the file names are stable enough to locate each item.

Severity counts: 0 critical, 2 high, 11 medium, 11 low, 1 info.

## Summary

DEPENDENCY AUDIT of /Users/johannes.blaschke/Developer/nsm_rs (Cargo.toml: 21 direct deps, Cargo.lock v4: 206 packages, vendor/: 205 crates / 356MB tracked in git, rustc 1.98.1, edition 2021, no rust-version).

Advisory exposure at the LOCKED versions (verified against rustsec.org per-advisory pages):
- rustls 0.23.20: RUSTSEC-2026-0285 (MEDIUM, TLS1.3 handshake encryption-level check; affected >=0.23.13 <0.23.45) -> AFFECTED. RUSTSEC-2024-0399 (Acceptor::accept panic) patched >=0.23.18 -> not affected.
- rustls-webpki 0.102.8 (transitive): RUSTSEC-2026-0049 (CRL DP matching; <0.103.10), -0098/-0099 (name constraints on URI / wildcard; <0.103.12), -0104 (CRL parse panic; <0.103.13) -> AFFECTED (CRL ones irrelevant, nsm uses no CRLs; name-constraint ones require CA misissuance).
- aws-lc-sys 0.24.0 (transitive, the only crypto provider actually compiled): RUSTSEC-2026-0045 (AES-CCM timing; >=0.14 <0.38), -0046/-0047 (PKCS7_verify bypass, HIGH; >=0.24 <0.38), -0048 (CRL DP; HIGH) -> AFFECTED per cargo-audit though unreachable via rustls (no PKCS7/CCM in TLS path). -0044 (>=0.32) not affected.
- h2 0.4.7: RUSTSEC-2026-0258 (unbounded empty DATA frames; patched >=0.4.16) -> AFFECTED and reachable: server advertises "h2" ALPN (src/tls.rs:96) and hyper-util "full" enables http2.
- tokio 1.42.0: RUSTSEC-2025-0023 (broadcast Sync unsoundness; patched 1.42.1/1.44.2) -> flagged, but nsm uses no broadcast channel.
- ring 0.17.8 sits in Cargo.lock+vendor (optional rustls feature, never compiled on any target: `cargo tree --target all -i ring` = nothing) -> RUSTSEC-2025-0009 (<0.17.12) will still be flagged by cargo-audit.
- rustls-pemfile 2.2.0: RUSTSEC-2025-0134 UNMAINTAINED (repo archived 2025-08-19); replacement is rustls_pki_types::pem::PemObject.
- No advisories / not affected: clap, env_logger, log, serde, serde_json, threadpool, lazy_static, hyper 1.x, http-body-util, hyper-util, hyper-rustls, tokio-rustls (2020-0019 is pre-0.13), url/idna 1.0.3 (2024-0421 patched >=1.0.0), base64 (2017-0004 ancient), pnet 0.33 (2019-0037 transport <0.27.2, 2020-0167 pnet_packet <0.28), mio 1.0.3, smallvec 1.13.2. openssl/native-tls/webpki-roots not in the compiled graph (webpki-roots 0.26.7 is in the lock only as an unused optional).

Build-correctness defects in Cargo.toml: `[features] ring = []` / `aws-lc-rs = []` (Cargo.toml:33-34) do not forward to rustls; `cargo check --offline --features ring` FAILS (E0433 at src/tls.rs:82, src/operations.rs:652, src/operations.rs:786). The build only works because hyper-rustls' default feature turns on rustls/aws_lc_rs. hyper's http1/http2/server/client features are only enabled via hyper-util "full" feature unification (hyper 1.x default = []).

Dead/redundant deps: threadpool (0 uses, last release 2020), rustls-platform-verifier (optional, 0 references, but drags duplicate rustls-native-certs 0.7.3 / security-framework 2.11 / core-foundation 0.9 / jni / android crates into lock+vendor), pki-types alias (Cargo.toml:21; operations.rs:41 already uses rustls::pki_types), lazy_static (2 statics; README says superseded by std LazyLock since 1.80), base64 (only to newline-join DER certs into Payload.root_ca; sending PEM makes it unnecessary), url (only form_urlencoded::parse x2 and Url::parse x2, pulls idna + ~14 ICU crates), pnet (only datalink::interfaces() at network.rs:32; pulls 35 crates incl. pnet_packet/pnet_transport/pnet_macros(syn 1.0.109 + regex) and winapi 0.3 = 106MB of vendor/).

Firm replacement verdicts: lazy_static -> std::sync::LazyLock (and drop the Arc/globals); threadpool -> remove; rustls-platform-verifier -> remove (reqwest 0.13 brings 0.7 by default later); log+env_logger -> tracing 0.1.44 + tracing-subscriber 0.3.23 (env-filter) with the `log` bridge for rustls; hand-rolled hyper servers (7 accept loops) + hyper_util::client::legacy (14 HttpsConnectorBuilder sites) -> axum 0.8.9 + reqwest 0.13.5 (rustls default, aws-lc, `tls_certs_only` for the private CA); clap builder -> clap derive, merging cli.rs into models.rs structs; pnet -> if-addrs 0.15.0 (libc / windows-sys 0.61 only); rustls-pemfile -> rustls::pki_types::pem::PemObject (no new dep); base64 -> remove, ship PEM text; std::io::Error -> thiserror 2.0.21 enum in the shared lib + anyhow >=1.0.103 in the two bins.

MSRV/edition: the latest versions raise MSRV to 1.85 (clap 4.6.x, hyper-rustls >=0.27.8, reqwest >=0.13.4, rustls-platform-verifier 0.7); everything else <=1.80. rustc 1.98.1 is fine; the Dockerfile's RUST_VERSION=1.83.0 (Dockerfile:9) is NOT and will break on bump. A dependency's own edition never constrains nsm; nsm itself should move to edition 2024 + rust-version = "1.85" (no unsafe/static mut in tree, `cargo fix --edition` is low risk).

Vendoring: .cargo/config.toml globally replaces crates-io with vendor/ (all 205 lock entries, all targets, all optional features: ~60 crates never compile on the host; winapi-* 106MB, windows-* ~90MB, aws-lc-sys 56MB, ring 13MB). 19 vendor churn commits; git pack is 329MB. Recommend: commit only Cargo.lock; CI runs `cargo audit`/`cargo deny` online and, on tag, `cargo vendor-filterer --platform x86_64-unknown-linux-gnu --platform aarch64-unknown-linux-gnu --format=tar.zstd` published as a release asset for air-gapped HPC; developers use `cargo fetch --locked` + `cargo build --locked --offline` (warm CARGO_HOME) or `cargo local-registry` 0.2.12; purge vendor/, docs/ (156MB rustdoc 1.78 from 2024-07-24 for a crate `nsm` binary that no longer exists), nsm-dev-buildx-latest.tar (18MB), src/.DS_Store and .env (another developer's cert paths) from history with git filter-repo.

## Detail

## Direct dependency table (locked -> latest stable, verified via crates.io API on 2026-09-24)

| crate | locked | latest stable (date, MSRV) | advisories at locked | verdict | reason |
|---|---|---|---|---|---|
| clap | 4.5.23 | 4.6.7 (2026-09-14, 1.85) | none | keep+bump, switch builder->derive | 4.6.0 only raised MSRV; derive lets models.rs structs double as CLI args and REST bodies (cli.rs:7, api_builder.rs:210/342/474/601 duplication) |
| env_logger | 0.11.6 | 0.11.11 (2026-06-25, 1.71) | none | replace with tracing-subscriber 0.3.23 | async multi-connection broker needs spans; env-filter keeps RUST_LOG |
| log | 0.4.22 | 0.4.34 (2026-08-22, 1.71) | none | replace with tracing 0.1.44 (keep `log` transitively for rustls; enable tracing `log` bridge) | every module has `#[allow(unused_imports)] use log::{...}` |
| pnet | 0.33.0 | 0.35.0 (2024-05-30, unspecified; still winapi 0.3) | 2019-0037 (transport <0.27.2), pnet_packet 2020-0167 (<0.28) - not affected | replace with if-addrs 0.15.0 | one call `datalink::interfaces()` network.rs:32; pnet subtree = 35 crates + winapi 114MB vendored + syn 1.0.109 |
| serde | 1.0.216 | 1.0.229 (2026-07-18, 1.56) | none | keep+bump | - |
| serde_json | 1.0.133 | 1.0.151 (2026-07-20, 1.71) | none | keep+bump | - |
| threadpool | 1.8.1 | 1.8.1 (2020-05-11) | none | remove | zero uses (only comments service.rs:396,484) |
| lazy_static | 1.5.0 | 1.5.0 (2024-06-21; upstream: replaced by LazyLock, no further updates) | none | remove -> std::sync::LazyLock / Mutex::const_new | operations.rs:53, service.rs:68 |
| tokio | 1.42.0 | 1.53.1 (2026-07-20, 1.71) | RUSTSEC-2025-0023 (unsound broadcast; patched 1.42.1) - not used | keep+bump; narrow `full` -> rt-multi-thread,macros,net,sync,time,io-util | - |
| hyper | 1.5.2 | 1.11.1 (2026-08-28, 1.63) | none for 1.x | keep+bump (becomes transitive under axum/reqwest); declare features explicitly | hyper 1 has default=[]; currently enabled only via hyper-util `full` |
| http-body-util | 0.1.2 | 0.1.5 (2026-08-12, 1.61) | none | keep+bump / transitive | - |
| hyper-util | 0.1.10 | 0.1.20 (2026-02-02, 1.64) | none | keep+bump but stop using `client::legacy` (reqwest) and narrow `full` | legacy client is a documented transition shim (hyperium/hyper#3891) |
| rustls-pki-types (alias `pki-types`) | 1.10.1 | 1.15.1 (2026-07-23, 1.60) | none | drop direct dep; use `rustls::pki_types` incl. `pem::PemObject` | operations.rs:41 vs tls.rs:4 inconsistency |
| rustls-native-certs | 0.8.1 (+0.7.3 dup via platform-verifier) | 0.8.4 (2026-06-01, 1.71) | none | drop direct dep; via hyper-rustls native-tokio or reqwest platform verifier | tls.rs:65 single use |
| rustls-platform-verifier | 0.3.4 (optional, never enabled) | 0.7.0 (2026-04-12, 1.85; needs rustls >=0.23.27) | none | remove | 0 references; drags 3 duplicate crate pairs + jni/android into lock/vendor |
| hyper-rustls | 0.27.5 | 0.27.10 (2026-09-20, 1.85) | none | keep+bump; set default-features=false and forward provider features | defaults: aws-lc-rs, http1, tls12, logging, native-tokio; http2 NOT default |
| rustls | 0.23.20 | 0.23.45 (2026-09-14, 1.71; 0.24.0-dev.1 needs 1.85) | RUSTSEC-2026-0285 AFFECTED (>=0.23.13 <0.23.45) | keep+bump NOW | pulls webpki ^0.103.14 and aws-lc-rs ^1.18 (both fix transitive advisories) |
| tokio-rustls | 0.26.1 | 0.26.5 (2026-09-04, 1.71) | none (2020-0019 ancient) | keep+bump | still needed for server TlsAcceptor |
| url | 2.5.4 | 2.5.8 (2026-01-05, 1.63) | none (idna 1.0.3 OK) | replace direct use with form_urlencoded 1.2.2 / http::Uri; transitive under reqwest | pulls idna + ~14 ICU crates for two Url::parse and two form parses |
| rustls-pemfile | 2.2.0 | 2.2.0 (2024-09-30; repo archived 2025-08-19) | RUSTSEC-2025-0134 UNMAINTAINED | replace with rustls_pki_types::pem::PemObject | tls.rs:29,41,55; operations.rs:234,305 |
| base64 | 0.22.1 | 0.23.1 (2026-08-04, 1.71; breaking) | none (2017-0004 ancient) | remove; ship PEM text in Payload.root_ca | operations.rs:240,311; tls.rs:111 |

## Notable transitive crates

| crate | locked | latest | advisories at locked | note |
|---|---|---|---|---|
| h2 | 0.4.7 | 0.4.19 (2026-08-24) | RUSTSEC-2026-0258 AFFECTED (<0.4.16), reachable (server ALPN h2, tls.rs:96) | hyper 1.11.1 requires ^0.4.14 |
| rustls-webpki | 0.102.8 | 0.103.15 (2026-08-21) | 2026-0049/0098/0099/0104 AFFECTED (patched 0.103.13+) | rustls 0.23.45 requires ^0.103.14 |
| aws-lc-rs / aws-lc-sys | 1.12.0 / 0.24.0 | 1.18.1 / 0.45.0 (2026-09-01) | aws-lc-sys 2026-0045/0046/0047/0048 AFFECTED (patched 0.38+); 0044 n/a (>=0.32) | only compiled provider; 56MB vendored; needs C compiler only (non-FIPS), pregenerated bindings for x86_64/aarch64 gnu+musl |
| ring | 0.17.8 (lock only, never compiled) | 0.17.14+ | RUSTSEC-2025-0009 (<0.17.12), 2025-0010 (<0.17 unmaintained) | in lock via rustls optional feature; cargo-audit flags it |
| idna | 1.0.3 | 1.x | RUSTSEC-2024-0421 patched >=1.0.0 - OK | goes away with url |
| webpki-roots | 0.26.7 (lock only) | 1.x | none | unused optional of hyper-rustls |
| mio / smallvec / socket2 | 1.0.3 / 1.13.2 / 0.5.8 | - | none applicable | - |
| openssl / native-tls | absent | - | - | pure-rustls stack confirmed |
| syn | 1.0.109 + 2.0.90 | - | - | syn 1 only from pnet_macros |
| windows-sys | 0.52.0 + 0.59.0 | 0.61 | - | collapses on bump |
| thiserror | 1.0.69 (transitive) | 2.0.21 | - | new direct dep recommended at 2.x |

Duplicates in Cargo.lock (`cargo tree -d`): core-foundation 0.9.4/0.10.0, security-framework 2.11.1/3.1.0, rustls-native-certs 0.7.3/0.8.1 (all three from rustls-platform-verifier 0.3.4), syn 1/2 (pnet_macros), windows-sys 0.52/0.59, regex appearing under both env_filter and pnet_macros.

## New dependencies proposed (all MSRV <= 1.85)

| crate | version | MSRV | purpose |
|---|---|---|---|
| axum | 0.8.9 (2026-04-14) | 1.80 | REST front-end (api.rs) + bind-port HTTP listeners; Router/State/Json/Form extractors replace api_builder.rs |
| axum-server (optional) or 20-line tokio-rustls accept loop | 0.7.x | - | TLS for axum with existing rustls ServerConfig |
| reqwest | 0.13.5 (2026-09-08) | 1.85 | outgoing HTTP(S): default rustls + aws-lc + rustls-platform-verifier; `tls_certs_only()` for private CA; pooling/timeouts; replaces hyper_util::client::legacy |
| tracing / tracing-subscriber | 0.1.44 / 0.3.23 | 1.65 | structured logging with per-connection spans; env-filter |
| thiserror / anyhow | 2.0.21 / 1.0.104 (>=1.0.103 for RUSTSEC-2026-0190) | 1.77 / 1.68 | typed errors in lib, context in bins |
| if-addrs | 0.15.0 (2026-02-08) | unspecified | interface IP enumeration; deps: libc (unix) / windows-sys 0.61 (windows) |
| form_urlencoded | 1.2.2 | 1.51 | only if NOT adopting axum (single dep percent-encoding) |
| cargo-audit, cargo-deny, cargo-vendor-filterer, cargo-machete | tools (CI only) | - | advisory gate, license/dup/source policy, filtered vendor tarball, unused-dep detection |

## Feature/provider fix (Cargo.toml)

```toml
[dependencies]
rustls        = { version = "0.23.45", default-features = false, features = ["std", "tls12", "logging"] }
tokio-rustls  = { version = "0.26.5",  default-features = false, features = ["tls12", "logging"] }
hyper-rustls  = { version = "0.27.10", default-features = false, features = ["http1", "tls12", "logging", "native-tokio"] }  # or drop under reqwest
[features]
default   = ["aws-lc-rs"]
aws-lc-rs = ["rustls/aws_lc_rs", "tokio-rustls/aws_lc_rs", "hyper-rustls/aws-lc-rs"]
ring      = ["rustls/ring",      "tokio-rustls/ring",      "hyper-rustls/ring"]
```
and one `install_default()` at the top of each main (currently tls.rs:81-84, operations.rs:651-654, 785-788).

## Vendoring / offline-HPC options compared

| option | what | pros | cons |
|---|---|---|---|
| status quo | vendor/ (356MB, all targets + all optional features) committed; `.cargo/config.toml` hard-replaces crates-io | clone-and-build with zero network | repo bloat (pack 329MB), every `cargo update`/`cargo add` needs a re-vendor commit, ~60 crates never compile on Linux/macOS, binary-ish blobs unauditable in review, config hijacks every developer's cargo |
| A. release-time filtered vendor tarball (`cargo vendor-filterer --platform=x86_64-unknown-linux-gnu --platform=aarch64-unknown-linux-gnu --tier=2 --format=tar.zstd`) | CI builds tarball on tag; HPC users extract and build with `--offline --locked` and `--config` source replacement | ~1/3 size, reproducible (SOURCE_DATE_EPOCH), filtered crates left as Cargo.toml stubs so lock still resolves, no history churn, auditable via checksums | one online step per release; extra tool (CoreOS-maintained) |
| B. registry cache + `--locked --offline` | `cargo fetch --locked` on a connected node, rsync `$CARGO_HOME/registry` (or `cargo local-registry` 0.2.12 -> `.crate` + index dir, `[source.x] local-registry`) | simplest for dev/CI; nothing generated in git; Cargo.lock is the single source of truth | cache layout is cargo-version-coupled; must refetch when the lock changes; not filtered by target |
| C. git LFS / submodule for vendor/ | keeps vendor out of main history | shrinks main repo | LFS often unavailable/quota-limited on institutional git; still all-target bloat |
| D. container-first (Apptainer/Podman from Dockerfile) | build online in CI, ship image | no vendoring at all; Dockerfile already uses `--locked` | HPC sites vary in container support; still need CI network |

Recommendation: A for air-gapped deploys + B for developers/CI; remove vendor/ and the `[source]` replacement from git; keep Cargo.lock committed and `--locked` in every build; purge vendor/, docs/, nsm-dev-buildx-latest.tar, src/.DS_Store, .env from history with `git filter-repo` in one coordinated rewrite.

## Verification commands run
- `cargo check --offline` OK; `cargo check --offline --features ring` FAILS (E0433 x3 at tls.rs:82, operations.rs:652, operations.rs:786); `--features aws-lc-rs` OK; `--features rustls-platform-verifier` OK (no-op).
- `cargo tree --offline -e features -i rustls` -> aws_lc_rs enabled only through hyper-rustls default.
- `cargo tree --offline --target all -i ring` -> nothing (ring never compiled); `-i winapi` -> only via pnet_sys/pnet_datalink; `--all-features -i rustls-native-certs@0.7.3` -> only via rustls-platform-verifier.
- Dependency counts: host default 176 crates; `--target all` 224; pnet subtree 35; lock 205 (excl. nsm) = vendor dir count 205.
- vendor/ top sizes (MB): aws-lc-sys 56, winapi-x86_64 54, winapi-i686 52, windows-sys-0.52 22, windows-sys 17, windows_x86_64_gnu 13, windows_i686_gnu 13, ring 13, linux-raw-sys 11, winapi 8.

## Suggested branch order for the dependency work
1. `deps/security-bump`: rustls 0.23.45, h2, tokio, aws-lc, webpki (cargo update), rust-version 1.85, Dockerfile RUST_VERSION -> 1.98 / alpine 3.22, fix [features] forwarding. No API changes.
2. `deps/remove-dead`: threadpool, rustls-platform-verifier, pki-types alias, lazy_static -> LazyLock, base64 -> PEM, rustls-pemfile -> PemObject.
3. `deps/pnet-to-if-addrs`: network.rs rewrite (+ unit tests on the filtering helpers).
4. `deps/tracing`: log/env_logger -> tracing.
5. `arch/lib-crate + axum + reqwest + clap-derive + thiserror`: with the architecture refactor (src/lib.rs, two thin bins), since it touches every HTTP site anyway.
6. `repo/devendor`: drop vendor/, docs/, tarball, .env; add CI (fmt, clippy, test, audit, deny, vendor-filterer on tag), gh-pages docs; history rewrite.

## Findings

Ids are `DEP` plus the finding number, in the order the reviewer reported them (not by severity).

| # | Severity | Category | Finding | Location |
|---|---|---|---|---|
| DEP1 | high | vulnerable-dependency | rustls 0.23.20 affected by RUSTSEC-2026-0285 (TLS 1.3 handshake encryption-level check) | `Cargo.toml:25` |
| DEP2 | medium | vulnerable-dependency | rustls-webpki 0.102.8 (transitive) has four 2026 advisories; rustls 0.23.45 requires ^0.103.14 | `Cargo.lock:1` |
| DEP3 | medium | vulnerable-dependency | aws-lc-sys 0.24.0 (the only compiled crypto provider) flagged by RUSTSEC-2026-0045/0046/0047/0048 | `Cargo.lock:1` |
| DEP4 | medium | vulnerable-dependency | h2 0.4.7 affected by RUSTSEC-2026-0258 and reachable: server advertises h2 ALPN | `src/tls.rs:96` |
| DEP5 | low | vulnerable-dependency | tokio 1.42.0 flagged by RUSTSEC-2025-0023 (broadcast channel unsoundness) | `Cargo.toml:17` |
| DEP6 | low | lockfile-hygiene | ring 0.17.8 is in Cargo.lock/vendor but never compiled; still trips RUSTSEC-2025-0009 in cargo-audit | `Cargo.lock:1` |
| DEP7 | medium | unmaintained-dependency | rustls-pemfile 2.2.0 is unmaintained (RUSTSEC-2025-0134); replace with rustls_pki_types::pem::PemObject | `src/tls.rs:29` |
| DEP8 | low | replaceable-dependency | lazy_static -> std::sync::LazyLock (or const-initialized tokio::sync::Mutex) | `src/operations.rs:53` |
| DEP9 | low | unused-dependency | threadpool 1.8.1 is declared but never used (last release 2020) | `Cargo.toml:15` |
| DEP10 | low | unused-dependency | rustls-platform-verifier is optional, never referenced, and duplicates rustls-native-certs/security-framework/core-foundation in the lock | `Cargo.toml:23` |
| DEP11 | medium | replaceable-dependency | pnet 0.33 pulls 35 crates (incl. winapi 0.3, syn 1, pnet_packet/transport) for one interfaces() call; replace with if-addrs | `src/network.rs:32` |
| DEP12 | low | replaceable-dependency | base64 is only used to smuggle DER certs into Payload.root_ca; ship PEM text and drop it | `src/operations.rs:240` |
| DEP13 | low | replaceable-dependency | url 2.5.4 (idna + ~14 ICU crates) used only for form_urlencoded::parse and two Url::parse calls | `src/api_builder.rs:11` |
| DEP14 | medium | build-correctness | [features] ring/aws-lc-rs are empty and do not forward to rustls: `--features ring` does not compile | `Cargo.toml:33` |
| DEP15 | low | feature-hygiene | hyper features rely on hyper-util "full" unification; hyper-util/tokio "full" over-enable features | `Cargo.toml:19` |
| DEP16 | info | replaceable-dependency | log + env_logger -> tracing + tracing-subscriber (firm recommendation) | `Cargo.toml:10` |
| DEP17 | medium | architecture-dependency | Hand-rolled hyper servers (7 accept loops) + hyper_util::client::legacy (14 connector sites) -> axum + reqwest (firm recommendation) | `src/tls.rs:9` |
| DEP18 | low | replaceable-dependency | clap builder API -> clap derive, unifying cli.rs with the models.rs input structs | `src/cli.rs:7` |
| DEP19 | medium | error-handling-dependency | std::io::Error used as the universal error type -> thiserror in the shared crate, anyhow in bins | `src/operations.rs:48` |
| DEP20 | medium | toolchain | Dockerfile pins Rust 1.83.0 and Alpine 3.18: below the MSRV of the target dependency set | `Dockerfile:9` |
| DEP21 | low | toolchain | No rust-version, edition 2021: set rust-version = 1.85 and move to edition 2024 | `Cargo.toml:4` |
| DEP22 | high | vendoring | Vendoring: .cargo/config.toml + 356MB vendor/ in git vendors all targets and all optional features; ~60 crates never compile | `.cargo/config.toml:1` |
| DEP23 | medium | repo-hygiene | Tracked artifacts: 156MB stale rustdoc, 18MB Docker image tarball, src/.DS_Store, another developer's .env | `.env:1` |
| DEP24 | medium | process | No dependency audit tooling or CI: cargo-audit/cargo-deny/cargo-outdated/cargo-machete absent, zero workflows | `Cargo.toml:1` |
| DEP25 | low | redundant-dependency | rustls-native-certs direct dependency is redundant with hyper-rustls native-tokio / reqwest platform verifier | `src/tls.rs:65` |

### DEP1. rustls 0.23.20 affected by RUSTSEC-2026-0285 (TLS 1.3 handshake encryption-level check)

**Severity:** high  
**Category:** vulnerable-dependency  
**Location:** `Cargo.toml:25`

Cargo.toml:25 `rustls = { version = "0.23", default-features = false }` resolves to rustls 0.23.20 in Cargo.lock. RUSTSEC-2026-0285 (2026-09-14, MEDIUM CVSS 5.3) affects >=0.23.13 <0.23.45: unencrypted TLS 1.3 handshake messages following key-change messages in the same record are accepted instead of aborting the connection. nsm terminates TLS on the server side via `TlsAcceptor::from(Arc::new(server_config))` (src/tls.rs:159, src/mode_api/operations.rs:46, src/operations.rs:451), so the affected code path is live. RUSTSEC-2024-0399 (Acceptor::accept panic) is already patched at 0.23.18.

**Fix:** `cargo update -p rustls` to 0.23.45 (latest stable as of 2026-09-14, MSRV 1.71). This also pulls rustls-webpki ^0.103.14 and aws-lc-rs ^1.18 which fixes the transitive advisories below. Re-vendor or drop vendoring (see vendoring finding).

### DEP2. rustls-webpki 0.102.8 (transitive) has four 2026 advisories; rustls 0.23.45 requires ^0.103.14

**Severity:** medium  
**Category:** vulnerable-dependency  
**Location:** `Cargo.lock:1`

Cargo.lock pins rustls-webpki 0.102.8 (pulled by rustls 0.23.20). RUSTSEC-2026-0049 (CRL distribution-point matching, patched >=0.103.10), RUSTSEC-2026-0098 (URI name constraints accepted, patched >=0.103.12), RUSTSEC-2026-0099 (name constraints on wildcard certs, patched >=0.103.12), RUSTSEC-2026-0104 (reachable panic parsing CRL onlySomeReasons, patched >=0.103.13). nsm does not use CRLs, and the name-constraint bugs require an already-misissued certificate, so practical impact is low, but cargo-audit will fail. Note nsm builds its own RootCertStore from a peer-supplied CA (src/tls.rs:115-119) so certificate validation correctness matters more than usual here.

**Fix:** Bump rustls to 0.23.45 which requires rustls-webpki ^0.103.14 (latest 0.103.15, 2026-08-21). No code changes needed; webpki is not used directly.

### DEP3. aws-lc-sys 0.24.0 (the only compiled crypto provider) flagged by RUSTSEC-2026-0045/0046/0047/0048

**Severity:** medium  
**Category:** vulnerable-dependency  
**Location:** `Cargo.lock:1`

`cargo tree --offline -i aws-lc-sys` shows aws-lc-sys 0.24.0 <- aws-lc-rs 1.12.0 <- rustls 0.23.20 (via hyper-rustls default feature `aws-lc-rs`, Cargo.toml:24). Advisories issued 2026-03-20: RUSTSEC-2026-0046/0047 PKCS7_verify chain/signature bypass (HIGH, affected >=0.24.0 <0.38.0), RUSTSEC-2026-0045 AES-CCM tag timing side channel (MEDIUM, >=0.14 <0.38), RUSTSEC-2026-0048 CRL DP scope logic (HIGH). RUSTSEC-2026-0044 (>=0.32) does not apply. rustls never calls PKCS7 or AES-CCM, so these are not reachable through nsm's TLS path, but they will fail any `cargo audit`/`cargo deny` gate.

**Fix:** Bump rustls -> 0.23.45 which requires aws-lc-rs ^1.18 (1.18.1 -> aws-lc-sys ^0.45.0, patched). aws-lc-sys needs only a C compiler for non-FIPS builds and ships pregenerated bindings for x86_64/aarch64 gnu+musl, so HPC/alpine builds remain feasible; if a target lacks a C toolchain, make `ring` genuinely selectable (see features finding).

### DEP4. h2 0.4.7 affected by RUSTSEC-2026-0258 and reachable: server advertises h2 ALPN

**Severity:** medium  
**Category:** vulnerable-dependency  
**Location:** `src/tls.rs:96`

Cargo.lock has h2 0.4.7 (<- hyper 1.5.2 <- hyper-util 0.1.10 `full`). RUSTSEC-2026-0258 (2026-08-18, low): empty DATA frames are queued without bound, allowing memory exhaustion; patched >=0.4.16. nsm's server config sets `server_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec(), b"http/1.0".to_vec()]` (src/tls.rs:95-97) and serves via `hyper_util::server::conn::auto::Builder` (src/api.rs:121, src/mode_api/operations.rs:108,121,322,336, src/operations.rs:617,630) which speaks HTTP/2 when hyper-util's http2 feature is on (it is, via `features = ["full"]` Cargo.toml:20). Clients only `enable_http1()` (src/tls.rs:127,134) so h2 is exposed without being needed.

**Fix:** `cargo update -p h2` (latest 0.4.19; hyper 1.11.1 requires ^0.4.14). Additionally either drop "h2" from alpn_protocols and disable hyper-util/hyper `http2` features, or keep http2 deliberately and add h2 to the audit watch list.

### DEP5. tokio 1.42.0 flagged by RUSTSEC-2025-0023 (broadcast channel unsoundness)

**Severity:** low  
**Category:** vulnerable-dependency  
**Location:** `Cargo.toml:17`

Cargo.toml:17 `tokio = { version = "1", features = ["full"] }` resolves to 1.42.0. RUSTSEC-2025-0023 (INFO/unsound): `tokio::sync::broadcast` clones values in parallel while only requiring T: Send; patched in 1.42.1 / 1.43.1 / 1.44.2. nsm uses `tokio::sync::{Mutex, Notify}` (src/operations.rs:35,38) and `tokio::spawn` (12 sites), not broadcast, so it is not exploitable here but will fail cargo-audit. `cargo tree -e features -i tokio` shows `full` enabling fs, io-std, process, signal, signal-hook-registry that nsm never uses (only tokio::{net,sync,time,spawn,task,io,main} appear in src).

**Fix:** Bump to tokio 1.53.1 (MSRV 1.71) and narrow features to `["rt-multi-thread", "macros", "net", "sync", "time", "io-util"]` to drop signal-hook-registry and the process/fs machinery.

### DEP6. ring 0.17.8 is in Cargo.lock/vendor but never compiled; still trips RUSTSEC-2025-0009 in cargo-audit

**Severity:** low  
**Category:** lockfile-hygiene  
**Location:** `Cargo.lock:1`

Cargo.lock lists ring 0.17.8 and vendor/ring (13MB) exists, yet `cargo tree --offline --target all -i ring` prints nothing: ring enters the lock only as rustls' optional `ring` feature dependency, which Cargo.lock records regardless of feature selection. RUSTSEC-2025-0009 (AES/QUIC panic under overflow checks, patched >=0.17.12) therefore shows up in any lockfile-based audit even though no ring code is built. The same lock-only status applies to webpki-roots 0.26.7, rustls-platform-verifier 0.3.4 (+jni, rustls-platform-verifier-android, security-framework 2.11.1, core-foundation 0.9.4, rustls-native-certs 0.7.3), bindgen 0.69.5 and clang-sys.

**Fix:** `cargo update -p ring` (>=0.17.14) as part of the bump, and remove the optional `rustls-platform-verifier` dependency so its subtree leaves the lock. Add a deny.toml with `[advisories] ignore` only for truly unreachable entries, documented with the reason.

### DEP7. rustls-pemfile 2.2.0 is unmaintained (RUSTSEC-2025-0134); replace with rustls_pki_types::pem::PemObject

**Severity:** medium  
**Category:** unmaintained-dependency  
**Location:** `src/tls.rs:29`

Cargo.toml:28 `rustls-pemfile = "2"`. RUSTSEC-2025-0134 (2025-12-05, INFO/unmaintained): all versions; GitHub repo archived 2025-08-19; README: "The main function of this crate has been incorporated into rustls-pki-types". Call sites: `rustls_pemfile::certs(&mut reader).collect()` src/tls.rs:29, `rustls_pemfile::private_key(&mut reader).map(|key| key.unwrap())` src/tls.rs:41, src/tls.rs:55, src/operations.rs:234-235 and :305-306. rustls-pki-types 1.15.1 (already a dependency at 1.10.1, pulled by rustls 0.23.45 as ^1.12) provides `PemObject::{from_pem_file, pem_file_iter, from_pem_slice, pem_slice_iter, from_pem_reader}` under the default `std` feature for CertificateDer, PrivateKeyDer and CertificateRevocationListDer.

**Fix:** Replace with `CertificateDer::pem_file_iter(&path)?.collect::<Result<Vec<_>, _>>()?` and `PrivateKeyDer::from_pem_file(&path)?` (via `rustls::pki_types::pem::PemObject`, no extra dependency); remove `rustls-pemfile` from Cargo.toml. Also remove the `pki-types` alias dependency (Cargo.toml:21) and use `rustls::pki_types::` consistently (src/operations.rs:41 already does; src/tls.rs:4 does not).

### DEP8. lazy_static -> std::sync::LazyLock (or const-initialized tokio::sync::Mutex)

**Severity:** low  
**Category:** replaceable-dependency  
**Location:** `src/operations.rs:53`

Cargo.toml:16 `lazy_static = "1.4.0"` (locked 1.5.0, last release 2024-06-21; upstream README: "This crate has been replaced by the LazyLock type, which is stable since Rust 1.80.0 ... this crate will no longer be updated"). Two uses: `lazy_static! { pub static ref GLOBAL_LAST_HEARTBEAT: Sem = Arc::new(Mutex::new(None)); }` src/operations.rs:53-55 (the Arc around a `static` is pointless) and `lazy_static! { static ref GLOBAL_MSGBODY: Mutex<MsgBody> = Mutex::new(MsgBody::default()); }` src/service.rs:68-70. Both are process-global mutable state that the shared TCP/REST backend should carry in `State` instead.

**Fix:** Short term: `static GLOBAL_LAST_HEARTBEAT: tokio::sync::Mutex<Option<Instant>> = Mutex::const_new(None);` (no lazy init needed) and `static GLOBAL_MSGBODY: LazyLock<Mutex<MsgBody>> = LazyLock::new(|| Mutex::new(MsgBody::default()));`. Preferred: move both into `State`/`AMState` and delete the statics. Remove lazy_static from Cargo.toml.

### DEP9. threadpool 1.8.1 is declared but never used (last release 2020)

**Severity:** low  
**Category:** unused-dependency  
**Location:** `Cargo.toml:15`

Cargo.toml:15 `threadpool = "1.8"`. `grep -rn threadpool src/` matches only two comments (src/service.rs:396 "use a worker from threadpool", src/service.rs:484 "holds event loop and threadpool"); no `use threadpool` anywhere. All concurrency is tokio tasks. threadpool's last release is 1.8.1 (2020-05-11); no advisories, but it is dead weight in Cargo.lock/vendor (num_cpus also comes only from it).

**Fix:** Remove `threadpool` from Cargo.toml and fix the two stale comments. Add `cargo machete` or `cargo udeps` to CI to catch this class of drift.

### DEP10. rustls-platform-verifier is optional, never referenced, and duplicates rustls-native-certs/security-framework/core-foundation in the lock

**Severity:** low  
**Category:** unused-dependency  
**Location:** `Cargo.toml:23`

Cargo.toml:23 `rustls-platform-verifier = { version = "0.3", optional = true }`. No `rustls_platform_verifier` identifier exists in src/ (`cargo check --offline --features rustls-platform-verifier` compiles but changes nothing). Because Cargo.lock is feature-independent, its 0.3.4 subtree is locked and vendored anyway: rustls-native-certs 0.7.3 (duplicating 0.8.1), security-framework 2.11.1 (dup of 3.1.0), core-foundation 0.9.4 (dup of 0.10.0), jni 0.19.0, rustls-platform-verifier-android, cesu8, combine. Latest is 0.7.0 (MSRV 1.85, requires rustls ^0.23.27).

**Fix:** Remove it. If OS-trust-store verification is wanted later, enable it through `hyper-rustls`'s `rustls-platform-verifier` feature, or adopt reqwest 0.13 which uses rustls-platform-verifier by default ("rustls roots features removed, rustls-platform-verifier is used by default"), which would also let the direct `rustls-native-certs` dependency (Cargo.toml:22, src/tls.rs:12,65) go.

### DEP11. pnet 0.33 pulls 35 crates (incl. winapi 0.3, syn 1, pnet_packet/transport) for one interfaces() call; replace with if-addrs

**Severity:** medium  
**Category:** replaceable-dependency  
**Location:** `src/network.rs:32`

Cargo.toml:12 `pnet = "0.33.0"`; the only use is `for iface in datalink::interfaces()` reading `iface.name` and `iface.ips` (src/network.rs:3-4, 32-57). `cargo tree -p pnet` = 35 unique crates: pnet_base, pnet_datalink, pnet_sys, pnet_packet, pnet_transport, pnet_macros (proc-macro, drags syn 1.0.109 + a second regex root), pnet_macros_support, ipnetwork, no-std-net, glob, and on Windows winapi 0.3.9 (`cargo tree --target all -i winapi` shows only pnet_sys/pnet_datalink) whose vendored binaries winapi-x86_64-pc-windows-gnu (54MB) + winapi-i686-pc-windows-gnu (52MB) + winapi (8MB) are ~30% of vendor/. pnet's latest is 0.35.0 (2024-05-30, 16 months stale, 126 open issues) and 0.35's pnet_sys/pnet_datalink still depend on winapi ^0.3.9. pnet_packet has a memory-safety history (RUSTSEC-2020-0167) and pnet_sys is raw FFI.

**Fix:** Replace with `if-addrs` 0.15.0 (2026-02-08, 28M downloads, MIT/BSD-3; deps: libc on unix, windows-sys ^0.61.2 on windows only): `for iface in if_addrs::get_if_addrs()? { iface.name, iface.ip() }` with `IfAddr::V4/V6` giving the same v4/v6 split. Rejected: `network-interface` 2.0.5 still depends on winapi 0.3 + cc build dep + thiserror; `nix::ifaddrs` is unix-only and far larger. Result removes ~34 crates, syn 1.x, the duplicate regex, and ~114MB of vendor/.

### DEP12. base64 is only used to smuggle DER certs into Payload.root_ca; ship PEM text and drop it

**Severity:** low  
**Category:** replaceable-dependency  
**Location:** `src/operations.rs:240`

Cargo.toml:29 `base64 = "0.22.1"` (latest 0.23.1, 2026-08-04, MSRV 1.71, breaking API). Uses: `general_purpose::STANDARD.encode(cert)` joined with "\n" at src/operations.rs:240-242 and :311-313 (publish/claim read a PEM file with rustls_pemfile, then re-encode DER to bare base64), and decode at src/tls.rs:109-119 (`certs.lines().map(|line| general_purpose::STANDARD.decode(line).unwrap())`) feeding `RootCertStore::add_parsable_certificates`. PEM is already base64-with-armor; the round trip adds a dependency and a panic-on-bad-input `unwrap`. Separately (security lens): this means the peer chooses the trust anchor the broker/client will use (src/service.rs:543 `setup_https_client(p.root_ca.clone())`), i.e. attacker-supplied roots.

**Fix:** Send the PEM file contents verbatim in `root_ca: Option<String>` and decode with `CertificateDer::pem_slice_iter(root_ca.as_bytes())` on the receiving side; remove `base64`. (If reqwest is adopted, base64 0.23 returns transitively, but no direct use remains.) Flag the trust-anchor-in-payload design to the security review.

### DEP13. url 2.5.4 (idna + ~14 ICU crates) used only for form_urlencoded::parse and two Url::parse calls

**Severity:** low  
**Category:** replaceable-dependency  
**Location:** `src/api_builder.rs:11`

Cargo.toml:27 `url = "2.5.2"`. Uses: `use url::form_urlencoded;` src/api_builder.rs:11 with `form_urlencoded::parse(v.as_bytes())` at :24 and :59; `url::Url::parse(&host.to_string())?` src/mode_api/operations.rs:181 and `.unwrap()` at src/operations.rs:435. `cargo tree -d` shows url -> idna 1.0.3 -> idna_adapter -> icu_normalizer/icu_properties/icu_provider/icu_locid/... (displaydoc, tinystr, zerovec, yoke, litemap, writeable, ~14 crates) purely for IDNA, which nsm never needs (hosts are IPs). Latest url 2.5.8 (2026-01-05).

**Fix:** If adopting axum+reqwest: drop the direct dep (reqwest requires url ^2.4 and axum's Form/Query extractors replace the manual parsing), so nothing to do beyond removal. If staying on raw hyper: depend on `form_urlencoded` 1.2.2 directly (single dep: percent-encoding) and parse `host` with `hyper::http::Uri` (already available) instead of `url::Url`.

### DEP14. [features] ring/aws-lc-rs are empty and do not forward to rustls: `--features ring` does not compile

**Severity:** medium  
**Category:** build-correctness  
**Location:** `Cargo.toml:33`

Cargo.toml:32-34 declares `[features] ring = [] aws-lc-rs = []` while `rustls = { version = "0.23", default-features = false }` (Cargo.toml:25) and `tokio-rustls = { ..., default-features = false }` (Cargo.toml:26). Code gates on these: `#[cfg(feature = "ring")] let _ = rustls::crypto::ring::default_provider().install_default();` at src/tls.rs:81-84, src/operations.rs:651-654 and :785-788. Verified: `cargo check --offline --features ring` fails with E0433 "cannot find `ring` in `crypto`" at src/tls.rs:82, src/operations.rs:652, src/operations.rs:786. The default build works only because hyper-rustls 0.27 default features enable `rustls/aws_lc_rs` (confirmed by `cargo tree -e features -i rustls`). `install_default()` is also called from three places instead of once in main, and `--features ring --features aws-lc-rs` would install two providers.

**Fix:** Make hyper-rustls `default-features = false, features = ["http1", "tls12", "logging", "native-tokio"]` and define `default = ["aws-lc-rs"]`, `aws-lc-rs = ["rustls/aws_lc_rs", "tokio-rustls/aws_lc_rs", "hyper-rustls/aws-lc-rs"]`, `ring = ["rustls/ring", "tokio-rustls/ring", "hyper-rustls/ring"]`, with `#[cfg(all(feature="ring", feature="aws-lc-rs"))] compile_error!`. Call `install_default()` exactly once at the top of each `main`. Keep aws-lc-rs as default (rustls default, FIPS-capable); `ring` becomes the real fallback for HPC hosts without a usable C toolchain.

### DEP15. hyper features rely on hyper-util "full" unification; hyper-util/tokio "full" over-enable features

**Severity:** low  
**Category:** feature-hygiene  
**Location:** `Cargo.toml:19`

Cargo.toml:19 `hyper = "1.4.1"` declares no features, but hyper 1.x has `default = []`; `cargo tree -e features -i hyper` shows http1/http2/server/client all coming from `hyper-util feature "full"` (Cargo.toml:20). hyper-util `full` = client, client-legacy, client-pool, client-proxy, client-proxy-system, http1, http2, server, server-auto, server-graceful, service, tokio, tracing; nsm needs only tokio + server-auto + client-legacy (+http1). tokio `full` enables fs, io-std, process, signal (unused). Removing hyper-util's `full` would silently break hyper's feature set unless hyper declares its own.

**Fix:** Declare `hyper = { version = "1.11", features = ["http1", "server", "client"] }` (add "http2" only if h2 is kept), `hyper-util = { version = "0.1.20", features = ["tokio", "server-auto", "client-legacy", "http1"] }` (or drop hyper-util entirely behind axum/reqwest), and narrow tokio as noted. Latest: hyper 1.11.1 (MSRV 1.63), hyper-util 0.1.20 (MSRV 1.64), http-body-util 0.1.5.

### DEP16. log + env_logger -> tracing + tracing-subscriber (firm recommendation)

**Severity:** info  
**Category:** replaceable-dependency  
**Location:** `Cargo.toml:10`

Cargo.toml:10-11 `env_logger = "0.11.3"`, `log = "0.4.21"` (locked 0.11.6/0.4.22; latest 0.11.11/0.4.34, no advisories). Every module does `#[allow(unused_imports)] use log::{debug, error, info, trace, warn};` (e.g. src/tls.rs:17-18, src/network.rs:7-8, src/connection.rs:21). Bins init with `env_logger::Env` (src/tcp.rs:28, src/api.rs:31). The broker is an async, multi-connection state machine (event_monitor / heartbeat_handler in src/service.rs) where per-connection context (service_id, key, peer addr) must currently be formatted into each message by hand; tokio/hyper/axum/reqwest/tower-http all emit `tracing` events natively, and rustls emits `log`.

**Fix:** Adopt `tracing = "0.1.44"` + `tracing-subscriber = { version = "0.3.23", features = ["env-filter", "fmt"] }` (MSRV 1.65; RUST_LOG semantics preserved via EnvFilter), enable tracing's `log` feature or `tracing-log` so rustls/hyper-rustls `log` output is captured, and wrap each connection/heartbeat task in `#[instrument]`/`info_span!("conn", %service_id)`. Cost ~8 extra crates (tracing-core, tracing-subscriber, sharded-slab, thread_local, nu-ansi-term, tracing-log, matchers). If the team wants minimal churn, simply bumping log/env_logger is acceptable; but for the planned architecture rewrite, tracing is the better fit.

### DEP17. Hand-rolled hyper servers (7 accept loops) + hyper_util::client::legacy (14 connector sites) -> axum + reqwest (firm recommendation)

**Severity:** medium  
**Category:** architecture-dependency  
**Location:** `src/tls.rs:9`

`use hyper_util::client::legacy::Client; // TODO: can we do without legacy?` src/tls.rs:9 and src/mode_api/operations.rs:26. The legacy client is explicitly a transition shim (hyperium/hyper#3891: "will eventually be deconstructed into more composable parts"). Duplicated `HttpsConnectorBuilder::new()...build()` sites: src/tls.rs:124,131; src/operations.rs:460,468,710,718,835,843; src/mode_api/operations.rs:141,148; src/service.rs:1087,1094,1279,1286 (14). Duplicated `service_fn` + `auto::Builder::new(TokioExecutor::new())` accept loops: src/api.rs:120-121; src/operations.rs:583,617,630; src/mode_api/operations.rs:83,108,121,289,322,336 (7), plus hand-written method/path dispatch in src/api.rs and src/connection.rs api_server. TLS accept is a manual `TlsAcceptor` loop (src/mode_api/operations.rs:44-46). Alternatives weighed: (a) stay on raw hyper + write one `http.rs` helper: smallest dep graph (+0), but keeps legacy Client and hand-rolled routing/extractors/timeouts; (b) axum 0.8.9 (MSRV 1.80; hyper ^1.1, hyper-util ^0.1.3, tower 0.5, matchit, axum-core, +~15 crates) + reqwest 0.13.5 (MSRV 1.85; default TLS is rustls with aws-lc and rustls-platform-verifier, `tls_certs_only(roots)` for the private CA; pooling, timeouts, redirects; brings url/base64/tower-http/hyper-rustls transitively).

**Fix:** Choose (b): axum Router with `State<AMState>`, `Json<Publish>`/`Form<..>` extractors replacing src/api_builder.rs manual HashMap parsing; serve TLS with `axum-server` (rustls 0.23) or a 20-line tokio-rustls accept loop + `TowerToHyperService(router)`; one `reqwest::Client` built once per root_ca in `tls.rs` replacing all 14 connector sites. hyper, hyper-util, hyper-rustls, http-body-util then become transitive and can leave Cargo.toml; tokio-rustls stays for the server acceptor. Keep the TCP transport on raw tokio sharing the `Message` codec; the 'common backend' is the service/state layer.

### DEP18. clap builder API -> clap derive, unifying cli.rs with the models.rs input structs

**Severity:** low  
**Category:** replaceable-dependency  
**Location:** `src/cli.rs:7`

Cargo.toml:9 `clap = "4.5.4"` (locked 4.5.23; latest 4.6.7, 2026-09-14; 4.6.0 changelog: only change is "Update MSRV to 1.85", no breaking changes). src/cli.rs:7 `use clap::{Arg, Command, ArgAction, ArgMatches};` builds a positional OPERATION plus shared flags and then hand-copies matches into the per-operation structs in src/models.rs (Publish/Claim/Collect/SendMSG each repeating `root_ca: Option<String>` at models.rs:64,102,143,173,203), while src/api_builder.rs re-implements the same extraction from JSON/form HashMaps (api_builder.rs:210,342,474,601). Derive with `#[derive(Parser)]` + `#[command(subcommand)] enum Operation { Publish(Publish), Claim(Claim), ... }` lets the models.rs structs carry both `clap::Args` and `serde::Deserialize`, so CLI and REST share one definition and validation.

**Fix:** Bump to clap 4.6 with `features = ["derive"]` (adds clap_derive -> heck; syn 2 and proc-macro2 already present via serde_derive) and delete the builder in src/cli.rs. Requires rust-version >= 1.85.

### DEP19. std::io::Error used as the universal error type -> thiserror in the shared crate, anyhow in bins

**Severity:** medium  
**Category:** error-handling-dependency  
**Location:** `src/operations.rs:48`

`pub type HttpResult = Result<Response<Full<Bytes>>, Error>;` with `use std::io::Error;` src/operations.rs:25,48; 15 `Error::new(ErrorKind::Other, ...)` constructions (src/connection.rs x3, src/operations.rs x7, src/service.rs x4, src/mode_tcp/operations.rs x1) coerce JSON, HTTP, TLS and protocol failures into io::Error strings; 130 `.unwrap()`/`.expect(` sites, e.g. `env::var("CERT_PATH").expect("CERT_PATH not set")` src/tls.rs:22 and `.map_err(|e| error!(...)).unwrap()` src/tls.rs:24-25 (logs then panics). thiserror 1.0.69 is already in the lock transitively. Latest: thiserror 2.0.21 (2026-09-23, MSRV 1.77), anyhow 1.0.104 (MSRV 1.68; use >=1.0.103 because RUSTSEC-2026-0190 `downcast_mut` unsoundness affects <1.0.103).

**Fix:** Add `thiserror = "2"` and define `enum NsmError { Io(#[from] std::io::Error), Json(#[from] serde_json::Error), Tls(#[from] rustls::Error), Http(#[from] hyper::Error), Pem(#[from] rustls::pki_types::pem::Error), Timeout, NoService{key}, Protocol(String), Config(String) }` in the future lib crate; add `anyhow = "1.0.104"` only in src/tcp.rs and src/api.rs `main`s. Map to HTTP status in one axum `IntoResponse` impl.

### DEP20. Dockerfile pins Rust 1.83.0 and Alpine 3.18: below the MSRV of the target dependency set

**Severity:** medium  
**Category:** toolchain  
**Location:** `Dockerfile:9`

Dockerfile:9 `ARG RUST_VERSION=1.83.0`, Dockerfile:15 `FROM rust:${RUST_VERSION}-alpine`, Dockerfile:49 `FROM alpine:3.18` (EOL May 2025). After bumping, clap 4.6.x (MSRV 1.85), hyper-rustls >=0.27.8 (1.85), reqwest >=0.13.4 (1.85) and rustls-platform-verifier 0.7 (1.85) will refuse to build (`package requires rustc 1.85`). Dockerfile:36 already uses `cargo build --locked --release` but does not use the vendor dir (no .cargo mount), so the image build is online-only; Dockerfile:20 installs clang/lld/musl-dev, sufficient for aws-lc-sys non-FIPS on x86_64/aarch64-unknown-linux-musl (pregenerated bindings). Dockerfile:73 CMD passes `--operation listen` while src/cli.rs uses a positional OPERATION (out of lens, noted).

**Fix:** Set `RUST_VERSION=1.98` (or `1.85` minimum) and `alpine:3.22`; add `rust-version = "1.85"` to Cargo.toml so the failure is explicit. Either copy `vendor/` + `.cargo/config.toml` into the build stage or (preferred) rely on `cargo fetch --locked` with a BuildKit cache mount as it does now.

### DEP21. No rust-version, edition 2021: set rust-version = 1.85 and move to edition 2024

**Severity:** low  
**Category:** toolchain  
**Location:** `Cargo.toml:4`

Cargo.toml:4 `edition = "2021"`, no `rust-version`. Toolchain is rustc 1.98.1 (2026-09-01). Highest MSRV among the recommended latest versions is 1.85 (clap 4.6.7, hyper-rustls 0.27.10, reqwest 0.13.5, rustls-platform-verifier 0.7.0); all others <=1.80 (axum 0.8.9: 1.80; tokio 1.53.1/rustls 0.23.45/aws-lc-rs 1.18.1/env_logger/log/base64 0.23: 1.71; thiserror 2.0.21: 1.77; anyhow: 1.68; serde 1.0.229: 1.56; hyper 1.11.1: 1.63; if-addrs: unspecified). A dependency's own edition (several of the 1.85-MSRV crates are edition 2024) never constrains a downstream 2021 crate; only MSRV does. For nsm itself, edition 2024 requires rustc >=1.85 (available) and the codebase has no `unsafe`/`static mut`, so the migration lints (RPIT capture rules, if-let temporary scope, `gen` keyword, never-type fallback) are low risk.

**Fix:** Add `rust-version = "1.85"` and `edition = "2024"`, run `cargo fix --edition && cargo clippy --fix`, and add a CI job on the pinned MSRV (`dtolnay/rust-toolchain@1.85`) plus `cargo +nightly udeps`/`cargo machete`.

### DEP22. Vendoring: .cargo/config.toml + 356MB vendor/ in git vendors all targets and all optional features; ~60 crates never compile

**Severity:** high  
**Category:** vendoring  
**Location:** `.cargo/config.toml:1`

.cargo/config.toml:1-5 `[source.crates-io] replace-with = "vendored-sources" / [source.vendored-sources] directory = "vendor"` is committed, so every developer's `cargo update`/`cargo add` is redirected to vendor/ and requires a re-`cargo vendor` commit (git log shows 19 commits touching vendor/ since 2024-09-05; pack size 329MB for a 5k-line project). `cargo vendor` copies every Cargo.lock entry for every target and feature: comparing the lock (205 crates) with `cargo tree --prefix none` on this host shows ~60 crates that are never built here (winapi + winapi-{i686,x86_64}-pc-windows-gnu = 114MB, windows-sys x2 + windows-targets + 7 windows_* = ~90MB, ring 13MB, bindgen/clang-sys/libloading, rustls-platform-verifier, jni, backtrace/gimli/object, redox_syscall, wasi ...); aws-lc-sys alone is 56MB. Cargo.lock is `version = 4` (needs cargo >=1.78). The Dockerfile does not even use vendor/. Alternatives: (A) release-time filtered vendor tarball via `cargo vendor-filterer --platform=x86_64-unknown-linux-gnu --platform=aarch64-unknown-linux-gnu --tier=2 --format=tar.zstd` (CoreOS-maintained; reproducible with SOURCE_DATE_EPOCH; filtered crates become Cargo.toml-only stubs so Cargo.lock still resolves), shipped as a GitHub release asset / kept in the HPC project directory, with the source replacement injected at build time (`cargo build --config 'source.crates-io.replace-with="vendored-sources"' --config 'source.vendored-sources.directory="vendor"' --offline --locked`) rather than committed; (B) `cargo fetch --locked` on a connected host, then `cargo build --locked --offline` from a synced CARGO_HOME (or `cargo local-registry` 0.2.12, 2026-03-25, which builds a .crate+index directory usable via `[source.x] local-registry = ...`); (C) git LFS/submodule for vendor/ (poor fit for HPC git servers); (D) container-only deploy (Apptainer from the existing Dockerfile), no vendoring at all. Tradeoffs: A gives auditable, ~100MB, platform-filtered, reproducible inputs for air-gapped nodes but needs one online CI step per release; B is simplest for developers/CI and version-locked by Cargo.lock but the registry cache layout is cargo-version-coupled and not filtered; committing vendor/ (status quo) gives the strongest 'clone and build' guarantee at the cost of repo size, churn and unauditable binary blobs.

**Fix:** Adopt A for HPC deploys and B for dev/CI: delete vendor/ and the `[source]` replacement from git (keep Cargo.lock, keep `--locked` everywhere), add `cargo-vendor-filterer` + `cargo audit` + `cargo deny check advisories bans licenses sources` to CI, publish the filtered tarball on tags, document `tar xf vendor.tar.zst && cargo build --offline --locked --config ...` in README. Rewrite history with `git filter-repo --path vendor --path docs --path nsm-dev-buildx-latest.tar --invert-paths` after coordinating with contributors.

### DEP23. Tracked artifacts: 156MB stale rustdoc, 18MB Docker image tarball, src/.DS_Store, another developer's .env

**Severity:** medium  
**Category:** repo-hygiene  
**Location:** `.env:1`

`git ls-files` shows: docs/ (156MB, generated by rustdoc 1.78.0 on 2024-07-24 per docs/nsm/index.html `data-rustdoc-version="1.78.0"`, documenting crate `nsm` with `fn.main.html` for a single binary that no longer exists; README.md:3 points GitHub Pages at it), nsm-dev-buildx-latest.tar (18,550,272 bytes; a `docker buildx` image export, added in 2d225eb4), src/.DS_Store (added 35e5352a), and .env:1-2 `CERT_PATH=/Users/sofiamorris/Desktop/certs/listen/cert.pem` / `KEY_PATH=...` (committed 2024-12-13). .dockerignore excludes `**/.env` and `**/.DS_Store` but .gitignore does not, so git tracks them. .gitignore:7-9 also contains typos (`view-events-rolebinding.yam`, `server.keyl`) so the intended ignores do not match.

**Fix:** Remove all four from the index, add `.env`, `.DS_Store`, `*.tar`, `docs/` to .gitignore (fix the typos), provide `.env.example`, and regenerate docs in CI (`cargo doc --no-deps --document-private-items` -> gh-pages branch via actions/deploy-pages) so the main branch carries no generated HTML. Purge from history together with vendor/ (see vendoring finding).

### DEP24. No dependency audit tooling or CI: cargo-audit/cargo-deny/cargo-outdated/cargo-machete absent, zero workflows

**Severity:** medium  
**Category:** process  
**Location:** `Cargo.toml:1`

`which cargo-audit cargo-deny cargo-outdated cargo-machete cargo-udeps cargo-msrv` -> none installed (only cargo-edit's cargo-add/rm/upgrade/set-version in ~/.cargo/bin); the repo has no .github/workflows, no deny.toml, no rust-toolchain.toml. The five 2026 advisories and one 2025 unmaintained notice above went unnoticed because nothing scans Cargo.lock. The vendored setup makes local `cargo audit` awkward (it reads Cargo.lock so it works offline with a fetched advisory DB, but the DB fetch needs network).

**Fix:** Add `.github/workflows/ci.yml` with `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, `cargo audit` (rustsec/audit-check), `cargo deny check` (deny.toml: advisories, bans with `multiple-versions = "warn"`, licenses allow-list, sources = crates.io only), and a monthly `cargo update` bot (Dependabot `package-ecosystem: cargo` or Renovate). Add `rust-toolchain.toml` pinning the channel used in CI.

### DEP25. rustls-native-certs direct dependency is redundant with hyper-rustls native-tokio / reqwest platform verifier

**Severity:** low  
**Category:** redundant-dependency  
**Location:** `src/tls.rs:65`

Cargo.toml:22 `rustls-native-certs = "0.8"` (locked 0.8.1, latest 0.8.4 2026-06-01). Used once: `let certs = load_native_certs().unwrap(); for cert in certs { root_store.add(cert).unwrap(); }` src/tls.rs:65-69 in the `None` branch of `load_ca`, while the client side already uses hyper-rustls' `.with_native_roots().unwrap()` (src/tls.rs:132) which does the same via hyper-rustls' `rustls-native-certs` feature. Note `load_native_certs()` in 0.8 returns a `CertificateResult` with partial errors that the `.unwrap()` pattern hides. Since 0.8.x rustls-native-certs is also what rustls-platform-verifier uses on Linux.

**Fix:** Drop the direct dependency: use hyper-rustls `with_native_roots()` (or, after moving to reqwest 0.13, its default rustls-platform-verifier) for both client and server-side root stores, and propagate `CertificateResult.errors` as warnings instead of unwrapping.


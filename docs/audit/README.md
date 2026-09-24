# NSM audit (2026-09-24)

Audit of `main` at `edd23a33`, produced as the first step of the [cleanup plan](../PLAN.md). Seven independent reviews were run, one per lens; the security lens was additionally re-checked finding by finding by three further reviewers (code reading, impact, compensating controls). No security finding was refuted; severities in that file are the reviewers' consensus.

| Lens | Findings | Critical | High | Medium | Low | Info |
|---|---|---|---|---|---|---|
| [Architecture and duplication](architecture.md) | 18 | 1 | 8 | 7 | 2 | 0 |
| [Wire protocol and state machine](protocol.md) | 32 | 5 | 11 | 10 | 5 | 1 |
| [Security](security.md) | 40 | 4 | 14 | 12 | 10 | 0 |
| [Correctness and concurrency](correctness.md) | 35 | 2 | 11 | 11 | 9 | 2 |
| [Code quality and idioms](quality.md) | 29 | 0 | 9 | 16 | 4 | 0 |
| [Dependencies](deps.md) | 25 | 0 | 2 | 11 | 11 | 1 |
| [Repository hygiene, infrastructure, docs and tests](hygiene.md) | 37 | 1 | 10 | 15 | 11 | 0 |
| **Total** | **216** | **13** | **65** | **82** | **52** | **4** |

Finding ids are the lens initial plus a number (`S1`, `P3`, `C11`). Many findings appear in more than one lens because the lenses were run independently; the per-lens files are kept complete rather than deduplicated so that each can be read on its own. The cleanup branches reference these ids.

## Baseline measurements

| Measure | Value |
|---|---|
| Source | 4,969 lines in 17 files, two binaries, no library target |
| Tests | 0 |
| CI | none |
| `cargo clippy` warnings | 160 (about 120 mechanical, 40 design-level; 5 are futures never awaited) |
| `cargo fmt --check` diffs | 719 |
| `cargo doc` warnings | 8 |
| `unwrap()` / `panic!` / `process::exit` in `src/` | 125 / 29 / 8 |
| `TODO` comments | 53 |
| Tracked files | 23,449 (12,449 under `vendor/`, 10,971 under `docs/`) |
| Working tree | `vendor/` 356 MB, `docs/` 156 MB, `nsm-dev-buildx-latest.tar` 18 MB |
| Git pack | 330 MB, dominated by committed `target 2/` and `target 3/` build outputs |
| Toolchain | rustc 1.98.1 locally; Dockerfile pins 1.83.0 |

## What the lenses agree on

**The two transports were never one program.** The `tcp` and `api` binaries declare the same nine modules and the same `main` dispatch; every operation then re-branches on `ComType` (30 sites) or on an `(Option<TcpStream>, Option<Request>)` pair (11 `panic!("Unexpected state")` arms). The architecture lens catalogues 31 duplicated blocks, the largest being seven copies of the HTTPS client construction, three copies of the hyper accept loop with optional TLS, and a four-by-eight matrix of hand-written JSON parameter extraction in `api_builder.rs`. The module graph has three import cycles, including the lowest I/O module depending on the top-level operations module through a process-wide `lazy_static`.

**Neither transport works end to end today.**

- `send` never delivers: the relay sets `MsgBody.id = 0` and the broker looks up heartbeats by `id`, which starts at 1 (`P1`, `C19`).
- HTTP two-sided heartbeats hit a handler that returns `400 Not implemented`; the broker then `unwrap`s the non-JSON body inside its monitor task, which dies and leaves a permanently claimable ghost record (`P3`, `S15`, `C21`).
- The REST control plane can never start: the positional `OPERATION` is `required(true)`, so clap exits before the `else` branch that would serve `0.0.0.0:8080` (`A11`, `C6`). All 640 lines of `api_builder.rs` are unreachable.
- A single failed heartbeat removes a party; the `FailCounter` ten-failure threshold is dead code (`P2`, `C5`).
- The re-claim of a client whose service died runs `claim()` on a *clone* of the state, never updates the client's `service_id`, and never tells the client (`P6`, `C1`).
- `State::add`'s reconnect scan can loop forever while holding the state lock (`P5`, `C12`).
- Three retry `sleep`s and one protocol reply are futures that are never awaited (`C3`).

**The broker and every party can be killed from the network.** The TCP accept loop awaits each handler inline and the handler `unwrap`s every parse, so one empty connection ends accepting for good (`S1`, `S2`, `P4`). Every party runs `process::exit(0)` on any read error on any inbound connection to its bind port, including an idle probe (`S3`). In the `api` binary, an unauthenticated request can arm a watchdog that exits the whole server (`S4`). Message framing treats "a read shorter than 1024 bytes" as end of message (`S13`, `P10`). There is no authentication beyond a user-chosen `u64` key sent in clear on TCP (`S6`), the party being verified supplies the CA that verifies it (`S21`), and every HTTPS client is built `https_or_http` (`S20`).

**Nothing could have caught this.** There are no tests and no library target for tests to link against; the `lazy_static` singletons and `process::exit` calls would make an in-process test harness impossible even if one existed (`H28` to `H31`).

**Dependencies are stale and partly misconfigured.** `rustls` 0.23.20 is affected by RUSTSEC-2026-0285 (fixed in 0.23.45); `h2` 0.4.7 by RUSTSEC-2026-0258 and the server advertises `h2` in ALPN; `aws-lc-sys` 0.24 and `rustls-webpki` 0.102 carry 2026 advisories fixed by the same bump; `rustls-pemfile` is unmaintained (`DEP1` to `DEP7`). The `ring`/`aws-lc-rs` features are empty, so `--features ring` does not compile and the default build has a crypto provider only through `hyper-rustls`' defaults (`DEP14`). `threadpool` is unused; `rustls-platform-verifier` is optional and never enabled; `pnet` pulls 35 crates for one call (`DEP9` to `DEP11`).

**Repository hygiene.** A private TLS key and CSR were committed in `01a90972` and remain reachable from three remote branches (`H1`). An 18 MB Docker image tarball, `src/.DS_Store`, another developer's `.env`, and 156 MB of two-year-old rustdoc for a binary that no longer exists are tracked (`H2`, `H3`, `H8`, `H9`). `.gitignore` has typos (`server.keyl`, `.yam`) that defeated its two most important entries. The Dockerfile `CMD` uses a flag the CLI no longer has and a macOS interface name (`H11` to `H13`).

## How the plan responds

The plan's target architecture (one library crate, transport trait, registry actor, per-party monitors, operations written once) is drawn directly from the architecture lens's proposed layering and the protocol lens's list of semantics to preserve. The security and correctness findings that are structural (panics on peer input, lock-across-await, framing, process exits, globals) are fixed by that rewrite; the ones that are policy (limits, TLS strictness, control-plane exposure, validation) are the hardening branch; the dependency table becomes the dependency branch; the hygiene change list, CI design and test strategy in the hygiene lens become branches 01, 05 and 06. Decisions the audit could not make for the maintainer (vendoring, license, history rewrite, key rotation) are listed in the plan's sections 2 and 3.

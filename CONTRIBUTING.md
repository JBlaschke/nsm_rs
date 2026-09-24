# Contributing

## Setup

- Rust stable (the toolchain in `rust-version`, once set, is the minimum).
- No network is needed to build or test: dependencies are vendored under
  `vendor/` and `.cargo/config.toml` points Cargo at them. Use `--offline` to
  be sure nothing reaches out.
- The default crypto provider (`aws-lc-rs`) needs a C compiler; if you do not
  have one, build and test with `--no-default-features --features ring`.
- Optional tools used by CI, all installable with `cargo install` or
  Homebrew: `cargo-deny`, `cargo-machete`, `cargo-llvm-cov` (needs
  `llvm-tools-preview`, or `LLVM_COV`/`LLVM_PROFDATA` pointing at an LLVM
  matching your `rustc`).

## Before you push

Run what CI runs (`.github/workflows/ci.yml`):

```bash
cargo fmt --all -- --check
cargo clippy --offline --all-targets -- -D warnings
cargo clippy --offline --all-targets --no-default-features --features ring -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --offline --no-deps
cargo test --offline
cargo test --offline --no-default-features --features ring
cargo test --offline --test stress -- --ignored
cargo deny --all-features check
```

The coverage job fails below the floor in `COVERAGE_FLOOR`
(`.github/workflows/ci.yml`). Measure locally with
`cargo llvm-cov --offline --summary-only`; when the measured value grows,
raise the floor. It never goes down.

## Layout and rules

[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) describes the modules and the
rules they follow. The short version:

- Nothing below `main` prints, panics on peer input or exits the process.
  `clippy::unwrap_used`, `expect_used`, `panic`, `unreachable`, `todo` and
  `unimplemented` are denied outside tests; use `?`, `Option` combinators or
  an explicit `Error`.
- Every public item has a doc comment (`missing_docs` is denied). Module docs
  (`//!`) explain what the module is for and the rules it keeps.
- No lock is held across an `.await`; every spawned task is owned by a handle
  or a `CancellationToken`.
- Operations live in `ops` and return typed results; the CLI and the REST
  control plane are front-ends and must not grow logic of their own.
- Import direction is downward only: `main → cli → rest/ops → broker/party →
  transport → protocol/net/tls → config/error`.

## Tests

- **Unit tests** go next to the code they test, in a `#[cfg(test)] mod tests`.
  Randomized tests use the seeded generator in `src/testing.rs`; keep them
  deterministic and include the iteration number in assertion messages.
- **End-to-end tests** (`tests/e2e.rs`) use the harness in `tests/common/`:
  `Cluster::start(transport)` gives a broker on an ephemeral loopback port
  with `Timing::fast()`, and `publish`/`claim`/`kill`/`wait_until` build the
  scenario. Add a scenario there when behaviour spans the broker and the
  parties.
- **Control plane** tests go in `tests/rest.rs`, **binary** tests in
  `tests/cli.rs` (they run the built `nsm` through `CARGO_BIN_EXE_nsm`), and
  load scenarios in `tests/stress.rs` behind `#[ignore]`.
- Tests must not depend on the host: select the loopback address with
  `-i 127.` rather than an interface name, bind port 0, generate
  certificates with `rcgen`, and give every async test a deadline.

## Changing the protocol

Any change to `protocol::Message`, the records it carries or the framing:

1. Update the variant docs in `src/protocol/message.rs` and the tables in
   [`docs/PROTOCOL.md`](docs/PROTOCOL.md).
2. Extend `all_variants()` and the shape tests in `message.rs` and `codec.rs`.
3. Bump `PROTOCOL_VERSION` if an older peer could not decode the new format
   (renamed or removed fields or variants, changed framing). Adding an
   optional field or a new variant does not need a bump.
4. Record the change under "Breaking changes" in [`CHANGELOG.md`](CHANGELOG.md)
   when it bumps the version.

## Changing dependencies

Dependencies are vendored (decision D1 in [`docs/PLAN.md`](docs/PLAN.md)).

1. Edit `Cargo.toml` or run `cargo update -p <crate>`; keep features narrow.
2. Re-vendor: `cargo vendor` (this rewrites every `.cargo-checksum.json`, so
   expect a large diff). When only removing crates, delete the directories
   that no longer appear in `Cargo.lock` instead.
3. Run `cargo deny --all-features check`; extend the license allow-list in
   `deny.toml` only for licenses you have read, and never ignore an advisory
   without a reason and a follow-up.
4. Commit the vendor change on its own (`chore: re-vendor dependencies`) so
   reviewers can skip it.

## Commits and branches

- Small commits grouped by topic, in the imperative with a conventional
  prefix (`feat(broker): ...`, `fix(transport): ...`, `test: ...`,
  `docs: ...`, `ci: ...`, `build: ...`, `chore: re-vendor ...`). The body
  says why, not what the diff already shows.
- Every branch tip must pass the checks above. Inside a branch, topic
  commits may depend on each other.
- Update [`CHANGELOG.md`](CHANGELOG.md) in the same change for anything a
  user would notice: a flag, a default, a route, a message, an exit code.

## Documentation

- `cargo doc` publishes the README as the crate front page plus every
  module's docs; keep `//!` module docs current when behaviour changes.
- The markdown guides in `docs/` and the README are rendered to HTML by
  `scripts/render-docs.sh` (pandoc) and published together with the rustdoc
  by the Pages workflow. Run the script locally to check a change renders.

## License

NSM is licensed under the BSD 3-Clause License ([`LICENSE`](LICENSE)). By
contributing you agree that your contributions are licensed under the same
terms.

# Peer address and typed `collect`: plan

> Written on 2026-09-27 against `main` at `1b76c288`, executed on the stacked
> branches `peer/01` to `peer/03`. Section 0 is the live status; when the
> branches are merged this file moves under [`history/`](history/) like the
> [2026 refactor plan](history/2026-refactor/PLAN.md) before it.

## 0. Status

| Branch | State | Notes |
|---|---|---|
| `peer/01-typed-collect` | done | `Role` in `protocol`; `collected` carries `role`; protocol version 2; `ops::Collected` is an enum; no command's output changes |
| `peer/02-pairing-updates` | done | the pairing is a `watch` channel; `Session::pairings`; `nsm claim` prints every pairing; three-process CLI test |
| `peer/03-peer-command` | in progress | |

Each branch builds on the previous one and passes the checks in
[`CONTRIBUTING.md`](../CONTRIBUTING.md) at its tip.

## 1. The problem

A script that needs the address of the service its client was paired with has
no reliable way to get it.

- `nsm collect PARTY` answers two different questions on one line of stdout:
  the last text a service received, or the service a client is paired with.
  The binary picks "text wins" (`src/main.rs`, the `Collect` arm), and the
  wire reply behind it (`collected`) carries both as optional fields with the
  comment that "exactly one is `Some` in practice". That invariant holds only
  because the broker never parks text on a client; nothing in the types or
  the CLI guarantees an address comes back.
- `nsm claim` prints the service address once, at registration, and never
  again. When the broker re-pairs the client after its service died, the
  running process learns the new address on its next heartbeat and keeps it
  to itself; a script that captured the first line holds a dead address.
- The REST control plane has neither problem: `POST /v1/collect` returns
  typed JSON fields and the job view re-reads the pairing on every `GET`.

## 2. Decisions

| # | Decision | Taken | If you disagree |
|---|---|---|---|
| P1 | **The reply says who answered.** `collected` gains a required `role` field (`"service"` or `"client"`). | Required, and `PROTOCOL_VERSION` becomes 2: a version-1 party's reply no longer decodes. Every deployment ships one binary, and the wire already broke twice during the refactor, so an inference fallback would only preserve a heuristic. | Make `role` optional and infer it from which field is set; no bump. |
| P2 | **`ops::Collected` is an enum keyed by role**, `Service { text }` or `Client { service }`, serialised with `role` as the tag. The library cannot produce the ambiguous state, and each variant grows independently later (a service that learns its client, a client that receives replies). | As stated. The REST response for a client no longer carries a `text` key (it was always `null`). | A flat struct `{ role, text, service }` keeps both keys and moves the "which field applies" question to every caller. |
| P3 | **`Role` moves to `protocol`** and is spelled `Service` / `Client`, matching the broker's `Party::Service` / `Party::Client` and the protocol guide's vocabulary. `party::Role` stays as a re-export. | Rename; the old serde spelling (`publisher` / `claimer`) was not on the wire anywhere. | Keep `Publisher` / `Claimer` and map the spelling in serde. |
| P4 | **A pairing is a stream, not a value.** `PartyState` keeps the pairing in a `tokio::sync::watch` channel; `Session::pairings()` hands out a receiver. `nsm claim` prints one stdout line per pairing: the first at registration, one more each time the broker re-pairs it. | As stated; the first line keeps the README's job-script contract, the last line is the current service. A repeated handle (same service) prints nothing. | Print only the first line and add `--follow`; or exit when the pairing changes. |
| P5 | **One verb per question.** New `nsm peer PARTY` prints exactly the paired service's `host:port`; `nsm collect PARTY` prints exactly a service's last text. Asking a service for its peer, or a client for text, is a run-time error (exit 1) with the party's role in the message. | As stated. Scripts that collected an address from a client switch to `peer`. | Keep the fallback on `collect` for a deprecation period, or add `collect --peer`. |
| P6 | **"Nothing yet" is distinguishable.** When the party answered but has nothing to report (`collect` before any text, `peer` before a pairing), the command prints a note on stderr and exits **3**. Exit 1 stays "the operation failed" and 2 is clap's usage error. | As stated. | Exit 0 with empty stdout. |
| P7 | **No new REST route.** `POST /v1/collect` already returns typed fields, now tagged with `role`, and the job view follows re-pairing; `peer` is a CLI presentation of the same operation, implemented as accessors on `Collected` in `ops` so the binary grows no logic of its own. | As stated. | Add `POST /v1/peer` returning `{"service": ...}`. |

## 3. Branches

### `peer/01-typed-collect`

- `protocol::Role` (`Service` / `Client`, serde lowercase, `Display` as
  `service` / `client`); `party::Role` re-exports it; every use renamed.
- `Message::Collected { role, text, service }`; `PROTOCOL_VERSION = 2`;
  shape and round-trip tests, the codec's random generator, `all_variants`.
- `PartyHandler` answers `collect` with its role.
- `ops::Collected` becomes the enum of P2; `ops::collect` builds it from the
  reply; the REST route returns it as is; the CLI keeps its current
  behaviour (text for a service, address for a client) so this branch
  changes no command's output.
- Docs: `docs/PROTOCOL.md` (version, types table, `collected`),
  `docs/REST_API.md` (`/v1/collect` response), `CHANGELOG.md`.

### `peer/02-pairing-updates`

- `PartyState.service` is a `watch::Sender<Option<ServiceHandle>>`;
  `set_service` reports whether the pairing changed; `PartyState::pairings`
  and `Session::pairings` return receivers.
- `nsm claim` prints every pairing (P4).
- Tests: the watch in `party`, an end-to-end re-pairing observed through the
  receiver, and a three-process CLI test that kills a service and reads the
  client's second stdout line.
- Docs: README (job scripts, `claim` row), `docs/ARCHITECTURE.md`,
  `CHANGELOG.md`.

### `peer/03-peer-command`

- `Command::Peer`; `Collected::text()` and `Collected::service()` accessors
  in `ops` with `Error::WrongRole`; `main` prints one thing per verb and maps
  "nothing yet" to exit 3 (P5, P6); `rest` maps `WrongRole` to 400.
- Tests: argument parsing, the accessors, the full CLI session (`peer` on
  both roles, `collect` on both roles, exit 3 before the first text,
  unreachable party).
- Docs: README (how it works, quickstart, command table, exit codes),
  `docs/ARCHITECTURE.md` (roles diagram, exit codes), `CHANGELOG.md`.

## 4. Out of scope

- A service learning which client holds it. The broker has the data
  (`claimed_by` and the client's bind address); a `client` field on the
  service's heartbeat and on `Collected::Service` would add it. Nothing
  above needs it.
- Text flowing from a service back to its client. `Collected::Client` would
  gain a `text` field; `collect` on a client would then stop being an error.
- Queueing beyond "last message wins".

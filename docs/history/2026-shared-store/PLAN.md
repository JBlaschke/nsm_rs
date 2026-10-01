# Shared store: plan and record

> This is the plan that drove the September 2026 shared-store work, kept as
> a record. It was written on 2026-09-27 against `main` at `84578926`,
> executed on the stacked branches `store/01` to `store/04`, and merged
> through [#12](https://github.com/JBlaschke/nsm_rs/pull/12). The status
> table in section 0 is final. The decisions in section 2 live on in
> [`ARCHITECTURE.md`](../../ARCHITECTURE.md) under "Design decisions" as D17
> to D20; the follow-ups in section 4 are still open. The plan before this
> one is the [peer-address and two-way text work](../2026-peer-text/PLAN.md).
>
> Four independent designs were compared before the plan was written, and
> three reviewers scored them. All four put the store at the broker and gave
> it to the client's claim; the decisions record where they differed and
> what was taken.

## 0. Status

| Branch | State | Notes |
|---|---|---|
| `store/01-broker-store` | done | store types and messages; one store per claim in the registry; the broker answers `store_relay`; `--max-store-bytes`; a party answered `store` with "unexpected" until `store/02` |
| `store/02-party-relay` | done | both parties relay `store` to the broker through one helper shared with `send`; `ops::store`; end-to-end tests over every transport and in ping mode; store traffic in the stress test |
| `store/03-cli-and-rest` | done | `nsm store get\|put\|delete\|list` with `--json` and exit 3 for an unset key; `POST /v1/store`; every stdout line in `main` goes through one writer, so a closed stdout is exit 1 instead of a panic |
| `store/04-conditional-writes` | done | `if_version` on put and delete (0: the key must not be set), compared before the budget; `applied` on `stored`, false for a mismatch, which changes nothing and takes no version; `--if-version` with exit status 4 and HTTP 409 for a mismatch; can be dropped without touching 01 to 03 |

Each branch builds on the previous one and passes the checks in
[`CONTRIBUTING.md`](../../../CONTRIBUTING.md) at its tip.

## 1. The request

> please create a shared key-value store that is shared between a service
> and a client.

Today the pair shares one text in each direction: `send` at either party
parks a text for its peer, the peer's next heartbeat carries it, `collect`
reads it, and a second text replaces the first. That is a doorbell, not
shared state. A job and the service it claimed have no place to keep named
values that both can read and update: a readiness flag, the path of an input
file, the step a computation reached. They would have to encode all of it in
one text and race each other to replace it.

The store is a small key-value map per pairing, readable and writable by both
parties through the same front-ends as `send` and `collect` (the library,
the CLI and the control plane), kept by the broker, which is the one place
that knows who is paired with whom.

## 2. Decisions

| # | Decision | Taken | If you disagree |
|---|---|---|---|
| S1 | **The broker holds the only copy.** A party keeps nothing; it relays each request to the broker with its id and token (`store` becomes `store_relay`), as `send` becomes `deliver`. | As stated. One copy under the registry mutex makes every operation atomic and linearizable without a sync protocol; ping-mode parties behind NAT reach it because they already dial the broker; heartbeats, `PartyState` and the monitor are unchanged. | Replicas at the parties, synced through heartbeats: reads without a round trip, but writes must still reach the broker to be ordered, and every key needs the pending/restore machinery of the inbox, a merge rule, frame splitting and a refill after re-pairing. Or the service holds the store and the client reaches it through the service's listener: the store dies with the service, and a ping-mode service behind NAT cannot be reached. |
| S2 | **A store belongs to the client's claim** (`ClientEntry.store`): created empty by `claim`, kept by `reclaim` across re-pairings, dropped when the client is removed. | As stated. D7 makes the claim the unit that lasts, and D16 already keeps a client's pending text across a re-pairing. An acknowledged write survives until the claim ends, the replacement service reads everything written before (the dead service's entries included), no data crosses from one client to the next claimer, and `remove`, `reclaim` and `drop_party` need no new code. | Pairing-scoped: emptied at every re-pairing, so acknowledged writes vanish when a service dies. Service-scoped: the store outlives its client, so the next claimer reads the previous client's data. |
| S3 | **Access follows the claim.** The client always, including between losing its service and being re-paired; a service while it holds the claim. An unclaimed service reads an empty store (`client: null`) and its writes are refused with deliver's text, `service <id> is not claimed`. A removed party fails the token check. | As stated. A service that starts first can poll for its first input and see "nothing yet" (exit 3) instead of an exit 1 it cannot tell from "unreachable", and it cannot seed state that a future, unknown client would inherit. | Refuse every operation at an unclaimed service, as `deliver` does; or give each service its own store before any claim (metadata claimers could read). |
| S4 | **Four operations**: get, put, delete (idempotent), list (an atomic snapshot of every entry with its value). The last writer wins per key unless the writer states a condition (S11). | As stated. | Add prefix queries, multi-key transactions or TTLs. |
| S5 | **Versions come from one broker-wide counter**, like party ids: every applied write (a put, or a delete that removed something) takes the next number. An entry's `version` is the number of its last write; a store's `revision` is the last number issued to it, 0 for a store never written. | As stated. A version names one state for the broker's whole life, so a conditional write can never succeed against the next claim's store because the numbers happen to match. | A per-store revision starting from 0 at every claim: easier to read, but numbers repeat across claims. |
| S6 | **The reply names the store**: `stored { client, revision, entries }`, where `client` is the id of the claim's client (`null` for an unclaimed service). | As stated, following D13: the reply says who answered. A service whose client was replaced by a new claimer can tell the two stores apart. Ids are not secrets. | Leave `client` out and let the follow-up that tells a service its client answer that question. |
| S7 | **Store keys are one shell word**: 1 to 128 characters from `A-Z a-z 0-9 . _ - : /`, not starting with `-`, checked by a `StoreKey` type (clap: usage error; REST: 400; wire: a decode error). Values are any UTF-8 text, including empty text and newlines. | As stated. `store list` prints one key per line that never globs, never needs quoting and never parses as a flag. A store key is not the rendezvous key (`--key`), and the docs say "store key" wherever both appear. | Any printable ASCII (then keys need quoting and `-x` parses as an option), or arbitrary UTF-8 with escaped list output. |
| S8 | **One limit: `--max-store-bytes`** per store (default 16384, allowed 256 to 32768), counting each entry as its JSON-encoded key and value plus 64 bytes; `listen` refuses a budget whose full reply would not fit its own `--max-frame-bytes`. | As stated. Counting encoded bytes bounds the largest reply as well as memory (escaping can make a value six times longer on the wire), and the ceiling keeps every reply inside the parties' default 64 KiB frame. There are at most `max_registrations / 2` stores (every client holds a distinct service), about 80 MiB accounted with the defaults, 64 per host. | Separate caps on entry count, key length and value length; a broker-wide total; raw bytes against a smaller default. |
| S9 | **Three new messages**, `store`, `store_relay` and `stored`, with the operation's fields next to `type` (`{"type":"store","op":"put","key":"step","value":"5"}`). `PROTOCOL_VERSION` stays 3. | As stated. New variants need no bump by the documented rule, no existing message changes, and the flat shape is also the REST body. | A nested `op` object; one variant per operation; a bump to 4 as a marker. |
| S10 | **One command group and one route.** `nsm store get PARTY KEY` prints the value (exit 3 when unset); `put PARTY KEY --value TEXT` prints the new version; `delete PARTY KEY` prints nothing and succeeds whether or not the key was set; `list PARTY` prints the keys, one per line. `--json` prints the reply as one JSON line, the body `POST /v1/store` returns. REST has one route with the operation in the body; an unset key is 200 with no entries, as `collect` answers `text: null`. | As stated: one verb per question (D15), stdout carries only the result (D5), one ops function behind one route. | `nsm kv`; top-level `nsm get`/`nsm put`; a positional VALUE; four routes; 404 for an unset key. |
| S11 | **Conditional writes on their own branch.** `if_version` on put and delete (0: the key must be absent). A mismatch is an answer, not a failure: `stored` with `applied: false` and the current entry; exit status 4; HTTP 409. This extends D15's list of exit statuses. | As stated, on `store/04` so it can be dropped. It is one comparison under the lock and the only way two writers can update a key without losing each other's changes. | Drop `store/04`: last writer wins, and a read-modify-write can lose an update. |
| S12 | **Nothing is pushed and nothing is persisted.** Heartbeats and ping replies carry nothing store-related; a party learns about changes by asking. The store lives in broker memory for the claim's lifetime. A write acknowledged before a `send` is visible by the time the peer receives that text, so "write, then notify" works. | As stated. The heartbeat path, the restore logic and the registry's purity stay as they are; a broker restart already loses every registration. | A `revision` field on heartbeats so parties can wait for changes (additive later); persisting stores to the broker's disk. |

## 3. Branches

### `store/01-broker-store`

- `protocol::types`: `StoreKey` (validated on parse and on decode, like
  `RegToken`), `StoreOp` (`get`, `put`, `delete`, `list`, tagged `op`),
  `StoreEntry { key, value, version }`, `Stored { client, revision, entries }`
  with accessors.
- `Message::Store`, `Message::StoreRelay`, `Message::Stored`; `kind`,
  `is_reply`, `all_variants`, the shape tests, the codec's random generator.
- `broker::store::Store`: entries, revision and the byte budget; pure, no
  clock, a `Debug` that never shows keys or values.
- `Registry`: the version counter, `ClientEntry.store` (created by `claim`),
  `Registry::store(from, op)` resolving the store the way `deliver` resolves
  the peer (S2, S3).
- `BrokerHandler`: `store_relay` with the token check and the registry call in
  one critical section.
- `Limits::max_store_bytes`, `--max-store-bytes` (with `--max-registrations`
  in the limit options; it has no effect on `serve`), and the frame check in
  `broker::listen`.
- Docs: `docs/PROTOCOL.md` (messages, types, `store_relay`, rules, sizes),
  `docs/ARCHITECTURE.md` (registry model, configuration, failure table),
  `CHANGELOG.md`, and the README's limits table (the `--max-store-bytes` row
  and the frame limit `listen` now needs), because the flag ships here.

### `store/02-party-relay`

- `PartyHandler`: one relay helper shared by `send` and `store` (registration
  check, id and token, the broker call), and the `store` arm.
- `ops::store(party, op, net) -> Result<Stored>` and the re-exports.
- Tests: party handler; end to end over every transport and in ping mode
  (sharing, re-pairing, the claim ending, an unclaimed service, tokens,
  concurrent writers, a full store, "write, then send"); stress traffic.
- Docs: `docs/ARCHITECTURE.md` (roles, components, security model),
  `docs/PROTOCOL.md` (sequences), `CHANGELOG.md`; remove the "parties do
  not relay `store` yet" caveat from `docs/PROTOCOL.md` and from the rustdoc
  of `Message::Store`.

### `store/03-cli-and-rest`

- `nsm store get|put|delete|list` with `--json`; `main` prints results
  through a writer that turns a closed stdout into exit 1 instead of a panic.
- `POST /v1/store`.
- Tests: parsing, `tests/cli.rs` (help, usage errors, the full session, exit
  3), `tests/rest.rs`.
- Docs: README (how it works, quickstart, command table, exit codes,
  job-script recipes), `docs/REST_API.md`, `docs/ARCHITECTURE.md`,
  `CHANGELOG.md`.

### `store/04-conditional-writes`

- `if_version` on `StoreOp::Put` and `StoreOp::Delete`; `applied` on `Stored`;
  the comparison in `broker::store`.
- `--if-version N` on `store put` and `store delete`; exit 4 with the current
  version on stderr; HTTP 409 with the reply and an `error` field.
- Tests: the comparison, concurrent increments lose no update, create-only,
  the CLI and REST mappings.
- Docs: README, `docs/PROTOCOL.md`, `docs/REST_API.md`,
  `docs/ARCHITECTURE.md`, `CHANGELOG.md`.

## 4. Out of scope

- Waiting for a change: a blocking `get` or a revision on heartbeats. Polling
  on exit 3 is the established pattern, and a request held open would sit
  inside four nested request timeouts and hold connection permits that pings
  need.
- Persistence across broker restarts; replication of the broker.
- A service's own store before any claim (metadata claimers could read).
- Recording who wrote an entry; TTLs; binary values; prefix or range queries;
  multi-key transactions beyond the snapshot `list` returns.
- `--stdin` or `--value-file` for large values.
- Authenticating callers on a party's listener (mutual TLS,
  [#7](https://github.com/JBlaschke/nsm_rs/issues/7)): anyone who can reach a
  party's bind address can read and write its store through it, as with
  `send` and `collect`. The store is not a place for secrets.
- Typing a relay failure: over TCP the party closes the connection (a 502 at
  the control plane), over HTTP it answers a 500 `nack` (a 400). This applies
  to `send` already; it should be fixed for both relays together.

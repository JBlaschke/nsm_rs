# Rendezvous keys as text: plan

> Written on 2026-10-04 against `main` at `bd4ab1b8` (after the discovery
> work, [#14](https://github.com/JBlaschke/nsm_rs/pull/14)), to be executed
> on the stacked branches `key/01` and `key/02`. Section 2 lists the
> decisions with what was taken and what to change if you disagree; section 3
> says what each branch contains; section 4 what is left out on purpose. The
> plan before this one is the [discovery work](history/2026-discovery/PLAN.md).

## 0. Status

| Branch | State | Notes |
|---|---|---|
| `key/01-key-newtype` | done | `protocol::Key` is a type of its own instead of an alias for `u64`; nothing changes on the wire, on the command line or on the control plane |
| `key/02-text-keys` | done | a rendezvous key is text: 1 to 64 characters from `A-Z a-z 0-9 . _ - : /`, not starting with `-`; a JSON string on the wire, an unsigned integer still decoded as its decimal text; protocol version 4 |

Each branch builds on the previous one and passes the checks in
[`CONTRIBUTING.md`](../CONTRIBUTING.md) at its tip.

## 1. The request

> the key is an int -- is there a way to make it a string (of fixed length
> -- with padding -- if that will speed things up behind the scenes?

Asked on 2026-10-04. The answer was yes to the string and no to the padding,
with three choices put to Johannes and approved the same day: the store key's
rule for what a key may be, with a cap of 64 characters; a JSON string on the
wire under protocol version 4, with an unsigned integer still decoded as its
decimal text; and `1234` and `01234` being different keys.

Today the rendezvous key is `pub type Key = u64` in `protocol::types`: a
party gives it to `--key`, `publish` and `claim` carry it to the broker, the
broker keeps it in every service and client record, `store_by_key` names it
as `rendezvous`, `nsm_mesh_data` echoes it as `nsm_key`, and the status
document lists the parties under it. The broker never looks a key up in a
map: a claim scans the services for the lowest unclaimed one under the key
(`Registry::lowest_unclaimed`), and a store by key scans the clients and then
the services (`Registry::resolve_key`); both compare keys for equality, which
for text checks the length before any byte. The only map keyed by key is the
per-key table of the status gauges, rebuilt for each scrape, and the key is
never a metric label (decision D21). So the type of the key decides what a
user may write, not how fast the broker answers.

Two steps, one branch each: give the key a type of its own while it is still
an integer, so that every place that handles one is named and typed, then
change what the type holds.

## 2. Decisions

| # | Decision | Taken | If you disagree |
|---|---|---|---|
| K1 | **A rendezvous key is text, with the store key's rule and a shorter cap.** `protocol::Key` wraps a `String` of 1 to `MAX_KEY_BYTES` (64) characters from `A-Z`, `a-z`, `0-9` and `. _ - : /`, not starting with `-`. The rule is checked wherever a key is built (`FromStr`, `TryFrom<String>`, deserialisation), so an invalid key is a usage error on the command line, a decode error on the wire and a 400 on the control plane, and no invalid key exists in any typed layer. Two keys are the same when their text is: `1234` and `01234` are different keys, `job-7` and `JOB-7` too; nothing is trimmed or folded. | As stated. The rule is the store key's (D17): one shell word that never needs quoting, never globs and never parses as a flag, and is safe in a URL and in JSON without escaping. The cap is lower because a key is a column of `nsm status` and of every party row, and 64 characters hold a job id, a user and a step with room; one validator serves both keys, with two caps. Equality by text is what every other key-like value in nsm does (store keys, hosts) and is the only rule that needs no explaining. | 128 as for store keys, so that one constant serves both; any UTF-8 (then quoting, escaping and normalisation questions arrive with it); trimming or case folding (then two spellings name one key and the documentation has to say which one is canonical). |
| K2 | **No fixed length and no padding.** A key is stored and compared as the text it is. | As stated. Padding speeds nothing up here: nothing indexes by key (the scans above compare for equality), the per-key table of the gauges orders text as well as numbers, every record already holds heap-allocated text for its addresses, and a key is touched at registration, at a store by key and at a status read, never on the heartbeat path. A padded key would also be visible: `--key job-7` and `--key "job-7   "` would have to mean one key, or the user would have to pad. The type stops being `Copy`; the handful of places that passed a key by value clone it or borrow it. | A secondary index from key to party ids in the registry, if a scan over `--max-registrations` entries ever shows in a profile; it works the same for text. An inline fixed buffer behind the type, invisible on the wire, if the clones ever matter. |
| K3 | **On the wire a key is a JSON string; an unsigned integer still decodes.** `key` in `publish` and `claim`, `rendezvous` in `store_by_key`, `nsm_key` in `nsm_mesh_data` and the `key` fields of the status document are JSON strings. Decoding accepts a string or an unsigned integer, which is read as its decimal text, so `1234` and `"1234"` are one key; a negative or fractional number, `null` or a string against the rule is a decode error. The broker and the parties always send strings. `PROTOCOL_VERSION` becomes 4. In the library, `From<u64> for Key` is that mapping, so tests and callers may keep writing integer keys. | As stated. A changed field type is a bump by the rule in `CONTRIBUTING.md`, and the version is what `nsm_build_info` and `/v1/status` report, so a mixed deployment can be seen. Accepting an integer keeps every party and script from before this change working against a new broker, with the keys they used naming the same keys as before (`--key 1234` then and now is one key). A party of this version against an older broker fails at registration with the broker's nack (`invalid message: ...`), as every version mismatch does. | Strings only, so that an old party fails against a new broker too (a job array mid-upgrade would notice). Sending an integer when the key is all digits (then the canonical form depends on the text, and `01234` could not be told from `1234`). |
| K4 | **Front-ends.** `--key` on `publish`, `claim` and `store` takes the text, the hidden compatibility `--key` of `collect` and `send` too; a key against the rule is a usage error (exit 2) with the rule in the message. `POST /v1/publish`, `POST /v1/claim` and `POST /v1/store` take `key` and `rendezvous` as strings, integers accepted as on the wire; a bad key is a 400 with the reason. Job views, `GET /v1/status` and `nsm status` show the key text, and the reserved entry `nsm_key` carries it as before. | As stated: both front-ends stay thin over the one type (its `FromStr` is the command-line parser, its `Deserialize` the wire's and the control plane's), and the number-or-string reading comes to the control plane for free. | Nothing here stands apart from K1 and K3. |
| K5 | **The type first, the text second.** `key/01` turns the alias into a newtype around the integer: every signature that handles a key says `Key`, nothing a user sees changes, the protocol stays at 3. `key/02` changes what the type holds and everything that follows from it. | As stated: the mechanical diff (every place that said `u64` for a key) is reviewed apart from the one that changes behaviour, and the first can land alone. | One branch with both. |

## 3. Branches

### `key/01-key-newtype`

- `protocol::types`: `pub struct Key(pub u64)` with `#[serde(transparent)]`,
  `Display`, `FromStr`, `From<u64>` and `From<Key> for u64`, in place of the
  alias; the docs say what a key is for.
- `Error::NoService(Key)` instead of `NoService(u64)`.
- Every signature that carried a key as `u64` carries a `Key`: the CLI
  (`publish`, `claim`, `store --key`, the hidden `--key` of `collect` and
  `send`), the REST bodies and job views, `ops`, the party state and
  session, the registry, the metrics rows and the monitor summary. Clap
  parses the flag through `FromStr`.
- Tests build keys with `Key::from` or `.into()`; the end-to-end harness and
  the registry's test helpers take `impl Into<Key>`, so integer literals in
  scenarios stay as they are. The codec's random messages and the registry's
  randomised tests wrap their integers.
- Nothing on the wire, on the command line or on the control plane changes;
  `PROTOCOL_VERSION` stays 3. Docs: this plan.

### `key/02-text-keys`

- `protocol::types`: `Key` holds a `String`; `MAX_KEY_BYTES` (64),
  `KeyError` (`a rendezvous key is 1 to 64 characters from A-Z a-z 0-9 . _
  - : / and does not start with -`), `TryFrom<String>`, `FromStr`,
  `AsRef<str>`, `as_str`, `From<Key> for String`; `From<u64>` is the decimal
  text. The validator is shared with `StoreKey` (one function, two caps).
  `Serialize` writes a string; `Deserialize` is a visitor that takes a
  string or an unsigned integer and refuses the rest with the rule in the
  message (K3).
- `Key` is no longer `Copy`: `Party::key` and `PartyState::key` return a
  reference; `Registry::resolve_key`, `Registry::store_by_key`,
  `lowest_unclaimed` and `ops::store_by_key` borrow the key; records, the
  mesh data, the rows and the job views own a clone.
- `PROTOCOL_VERSION` is 4, with the row in the version table of
  `message.rs`.
- `MeshData::largest` uses the longest key; the test that pins the broker's
  entries under `REPLY_OVERHEAD` still holds (a 64-character key adds about
  a hundred bytes to a reply that had room to spare).
- Tests: types (the rule with its edge cases, both wire forms, an integer
  and its text decoding to one key, `1234` against `01234`, the random
  agreement test as for store keys), the codec's random keys as text,
  registry and handler with text keys, CLI parsing (`--key job-7`, a key
  against the rule is a usage error with the rule in the message, the
  legacy flags), `tests/cli.rs` (a session under a text key, the usage
  error), end to end under a text key over every transport, REST (a string
  key through publish and claim and in the job view, an integer key
  accepted and shown as text, a bad key 400), admin (the status document
  with a text key).
- Docs: `docs/PROTOCOL.md` (version 4 and its history line, the types
  table, every example that carries a key, the rules), `docs/REST_API.md`
  (the body tables, the examples, the store body), `docs/MONITORING.md` (the
  status examples), README (the key in the command reference and the store
  section), `docs/ARCHITECTURE.md` (the types paragraph, the security model,
  the decisions once filed), `CHANGELOG.md` (under "Breaking changes": the
  wire protocol is version 4 and the key is text), the module docs of
  `message.rs` and `types.rs`, this plan's status table.

## 4. Out of scope

- An index by key in the registry (K2): nothing has shown the scans in a
  profile.
- Keys beyond the rule: UTF-8, spaces, more than 64 characters (K1).
- Normalising keys: trimming, case folding, leading zeros (K1).
- A version handshake: the broker still answers a message it cannot decode
  with a nack, and a party exits on it; `nsm_build_info` and `/v1/status`
  say which version a broker speaks.
- `PartyId` and `StoreKey` are unchanged.

# Discovery by key: plan

> Written on 2026-10-02 against `main` at `4f895335` (after the monitoring
> work, [#13](https://github.com/JBlaschke/nsm_rs/pull/13)), to be executed
> on the stacked branches `discover/01` to `discover/03`. Section 2 lists the
> decisions with what was taken and what to change if you disagree; section 3
> says what each branch contains; section 4 what is left out on purpose. The
> plan before this one is the [monitoring work](history/2026-monitoring/PLAN.md).

## 0. Status

| Branch | State | Notes |
|---|---|---|
| `discover/01-optional-bind-port` | planned | `--bind-port` is optional on `publish` and `claim`: omitted, the operating system picks a free port when the heartbeat listener is bound |
| `discover/02-mesh-data` | planned | the reserved `nsm_` store keys: where the parties of a claim listen, answered by the broker from its registry to whichever party relays the read, as one JSON value under `nsm_mesh_data` and one entry per field; `list` shows them |
| `discover/03-store-by-key` | planned | `store_by_key`: a store operation addressed to the broker by rendezvous key; `nsm store ... ADDR --key RENDEZVOUS [--party-id ID]`; `POST /v1/store` with `broker` and `rendezvous` |

Each branch builds on the previous one and passes the checks in
[`CONTRIBUTING.md`](../CONTRIBUTING.md) at its tip.

## 1. The request

> Currently every usage model of NSM has been manually specify NSM's internal
> port (`--bind-port`) for heartbeats and so forth. I want to keep that as a
> feature but also add more lazyness:
>
> 1. if `--bind-port` is not specified, then search for a high-numbered port;
>    the central orchestrator should keep the discovered port in the key-value
>    store so that future interactions with the same workflow (service/client)
>    key can retrieve that information
> 2. if claim, publish, and store specify the central orchestrator instead of
>    a local address, then let the orchestrator "fill in" those discovered
>    ports. Eg:
>    1. `publish` without `--bind-port` discovered a HB port, and tells the
>       orchestrator, similarly for `claim`
>    2. while the `peer` `send` `collect` noun might not work without knowing
>       the HB port (those are meant to talk directly to a local port after
>       all)
>    3. `store get <address of the orchestrator, not the local port>
>       nsm_mesh_data` should return a json containing all relevant
>       information
>    4. `nsm_service_address` and `nsm_service_port` should be the service
>       address and port
>    5. as well as the heartbeat data should be `nsm_mesh_service_address`
>       and `nsm_mesh_service_port` should be the service's (`publish`) local
>       hb address and port
>    6. likewise `nsm_mesh_client_address` and `nsm_mesh_client_port`

Today every party chooses its own heartbeat port (`--bind-port` is required
on `publish` and `claim`), and a script that wants to reach a party afterwards
(`send`, `collect`, `peer`, `store`) must know it: either because it chose a
fixed port, which collides when two jobs share a node, or by parsing the
`(heartbeats on ...)` line the party prints on stderr. The broker knows every
party's heartbeat address, since it is the `bind_addr` of the registration,
and it already answers store operations, but only through a party and with
that party's token: nothing lets a script that knows only the rendezvous key
and the broker's address ask where the parties are.

Three steps, one branch each: let a party not choose a port (the operating
system picks one), let the broker say where the parties of a claim listen
(one JSON value, read like any other store entry), and let the broker answer
a store operation addressed by rendezvous key, so that a script needs the
key and the broker's address and nothing else.

## 2. Decisions

| # | Decision | Taken | If you disagree |
|---|---|---|---|
| L1 | **`--bind-port` is optional on `publish` and `claim`.** Omitted means 0: the operating system picks a free port from its ephemeral range when the heartbeat listener is bound, as `--bind-port 0` does today. `listen` keeps a required `--bind-port`: the broker is the one fixed, well-known address. | As stated. Binding port 0 is atomic: no scan, no retry, no race between finding a free port and taking it. The kernel's ephemeral range is high-numbered on Linux (32768 to 60999) and macOS (49152 to 65535) and is the site's to set (`net.ipv4.ip_local_port_range`). The party still prints the address it bound on stderr, and the broker records it (L2). | A scan: `--bind-port-range LO-HI`, trying the ports of the range in random order until one binds, for sites whose firewalls open a fixed range towards compute nodes. Additive later; `--bind-port PORT` keeps working for a chosen port. |
| L2 | **The broker already holds the discovered address.** `publish` and `claim` carry the party's actual `bind_addr` (admission refuses port 0), and the registry keeps it for the party's lifetime. Nothing is written into a store as an entry: `nsm_mesh_data` (L3) is projected from the registry when it is read. | As stated. The registry is the one place that is always current: a re-pairing changes which service a client's data names, and a projection follows at once, costs no store budget, cannot be overwritten or deleted by a `put` or a `delete`, and needs no new code in `claim`, `reclaim` or `remove`. | Write the addresses into the claim's store as ordinary entries at registration and at every re-pairing: visible to `list`, but stale between a service's death and the re-pairing, counted against the budget, and deletable. |
| L3 | **The broker's entries are reserved `nsm_` keys of every store.** `get nsm_mesh_data` answers the JSON of L4 as one entry, and every field of it that is set is an entry of its own under the field's name (`get nsm_service_port` answers `9000`); all at version 0 (the versions of writes start at 1), none stored or counted against the budget, through either party as every read today and by key (L5). `list` carries them beside the stored entries, in one key order. The prefix `nsm_` is reserved: a `put` or a `delete` of such a key is refused (`nsm_mesh_data is reserved: store keys starting with nsm_ are the broker's`), and a `get` of an `nsm_` key the broker does not know answers no entry; a field that is `null` has no entry, so a `get` of it is "not set" (exit 3) until that side registers. | As stated, revised after review on 2026-10-02: the first cut had `nsm_mesh_data` alone and kept it out of `list`; Johannes expected the six names to be keys and asked for a way to list the special keys. One command (`nsm store get`), one route (`POST /v1/store`), `--json` and the exit statuses for free; a shell script reads one field without a JSON parser and polls a missing side with exit 3 as it polls any key. Version 0 marks the entries as nothing that was written. | A verb of its own (`nsm mesh BROKER --key K`, a `lookup` message and route): typed, no reserved keys, but one more verb and one more route to learn, and not what was asked for. Or a `list` flag (`--all`) that keeps the broker's keys out of the default listing, if scripts that iterate `list` or test it for emptiness need that. |
| L4 | **The value is one flat JSON object**: the six fields the request names, `nsm_service_address` and `nsm_service_port` (the service's data-plane endpoint, what `claim` prints), `nsm_mesh_service_address` and `nsm_mesh_service_port` (where the service listens for heartbeats) and `nsm_mesh_client_address` and `nsm_mesh_client_port` (where the client does); plus `nsm_key`, `nsm_service_id` and `nsm_client_id`, and each endpoint once more as one string ready for the command line: `nsm_service` (`host:port`), `nsm_mesh_service` and `nsm_mesh_client` (with the transport, `http://10.0.0.6:41232`, as `send`, `collect` and `store` take them). A side that is not there is `null`: the client's fields at a service nobody holds, the service's at a client whose service died and that is not re-paired yet. In the library: `protocol::MeshData` and `Stored::mesh_data`. | As stated. The six fields are the request's; the key and the ids say which workflow and which parties answered (the ids are what `--party-id` takes, L5); the combined strings save a script the scheme and the IPv6 brackets. Flat `nsm_*` names, as asked; `null` rather than a missing field, as every `Option` on the wire. The broker's entries at their largest fit the reply overhead, which grows from 1024 to 4096 bytes so that a `list` of a full store still fits `--max-store-bytes` plus the overhead (the smallest frame a broker may run with is now 4352 bytes, 20480 with the default budget); a test pins that. | Only the six fields; nested objects (`service`, `mesh.service`, `mesh.client`); leaving null fields out. |
| L5 | **A store operation may be addressed by rendezvous key at the broker.** One new message, `store_by_key { rendezvous, party_id, op }` (operator to broker), answered with `stored` or `nack` exactly like a relay. The broker resolves the key to one party: the one client under the key (so its claim's store), or, when no client is under it, the one service (which reads an empty store and may not write, as today); with `party_id`, that party, which must be under the key. Two or more clients, or no client and two or more services, are refused with the candidates listed. | As stated. One client per key is the workflow the request describes, and a spare service beside the claim is the failover recipe, which stays unambiguous. Refusing an ambiguous key names the ids, and `party_id` (either side of a claim) picks one. The resolution ends in `Registry::store` as it is: a client reaches its own store, a service its holder's. | Pick the lowest id silently; answer every claim of the key at once (a different reply shape); require `party_id` always. |
| L6 | **The rendezvous key is treated as the capability it already is.** Whoever knows a key can claim a service under it, or publish a service under it and so be paired with the key's next client or its orphaned one; letting the key read and write that key's store adds no power the key did not give. What changes is where the store can be reached from: the broker's address is fixed and reachable by every party, while a party's own listener may not be (ping mode, NAT). The security model says so. | As stated. The store was never a place for secrets, and the key already decides who takes part in a workflow. | Reads only by key and writes only through a party; or a broker policy flag (`--no-store-by-key`) that refuses the message. Both are additive. |
| L7 | **CLI**: `nsm store get\|put\|delete\|list ADDR [STORE_KEY] [--key RENDEZVOUS [--party-id ID]]`. Without `--key`, `ADDR` is a party's heartbeat address as today; with it, `ADDR` is the broker's. Printing, `--json` and the exit statuses (3 for an unset key, 4 for a missed condition) are the same either way; a refusal by the broker (no party under the key, an ambiguous key) is exit 1 with its reason. In the help the positional store key is `<STORE_KEY>` and the address `<ADDR>`. | As stated. `--key` is the rendezvous key everywhere else (`publish`, `claim`, and the hidden compatibility flag of `collect` and `send`), and a job script has it in a variable already. `<STORE_KEY>` in the usage line and "store key" in every text keep the two apart, as the documentation already does. | `--rendezvous` for the flag, so that "key" means one thing inside `nsm store`; or `--broker ADDR` instead of giving the positional two meanings. |
| L8 | **REST**: `POST /v1/store` takes `{"broker":...,"rendezvous":K,"party_id":n,...op}` as well as `{"party":...,...op}`: exactly one of the two forms, anything else 400. The reply and the status codes are unchanged (an unknown or ambiguous key is a 400 with the broker's reason). | As stated: one route, one `ops::StoreTarget` behind it (a party's address, or the broker's with the key), one `ops::store` and one `ops::store_by_key`. | A second route, `POST /v1/store-by-key`. |
| L9 | **Nothing bumps the protocol.** `store_by_key` is a new variant and `nsm_mesh_data` a reserved key, so `PROTOCOL_VERSION` stays 3. A broker that predates this answers `store_by_key` with `unexpected store_by_key at the broker` and treats `nsm_mesh_data` as an ordinary key; a party nacks `store_by_key` as unexpected, as it does `store_relay`. `nsm_requests_total` gains `kind="store_by_key"`, and operations by key count in `nsm_store_ops_total` like relayed ones. | As stated, by the compatibility rule in `PROTOCOL.md` section 6. The dashboard groups by `kind` and needs no change. | A bump to 4 as a marker. |
| L10 | **`send`, `collect` and `peer` keep taking a party's address.** The request says so: they talk to a party's own listener. The mesh data gives a script those addresses (`nsm_mesh_client`, `nsm_mesh_service`), so the lazy flow is complete without changing them. | As stated; the smallest change that works end to end. | Let the broker forward `send` and `collect` to the party by key (one more hop, with a reply the broker must wait for), or answer `peer` by key from the registry (the handle is `nsm_service`). Follow-ups in section 4. |

## 3. Branches

### `discover/01-optional-bind-port`

- `cli`: `--bind-port` on `publish` and `claim` defaults to 0, with help
  saying that 0 or omitted lets the operating system pick; `listen` is
  unchanged.
- `main`, `ops`, `party`: nothing; 0 already means a free port, and the
  party prints what it bound.
- Tests: parsing with the flag omitted; `tests/cli.rs` runs its sessions
  without `--bind-port`; the help text.
- Docs: this plan; README (command table, the address notes, the HPC
  notes), `docs/REST_API.md` (one sentence), `CHANGELOG.md`.

### `discover/02-mesh-data`

- `protocol::types`: `StoreKey::is_reserved` (the `nsm_` prefix),
  `StoreKey::mesh_data()`, `MeshData` (serde, the fields of L4 in that
  order) with `entry()` (the JSON under `nsm_mesh_data`) and `entries()`
  (that entry plus one per field that is set, all at version 0, in key
  order), `Stored::mesh_data()`.
- `Registry::mesh_data(from) -> Option<MeshData>`: for a client, itself and
  its service (none while orphaned); for a service, itself and the client
  holding it (none while unclaimed). `Registry::store` finds the store's
  owner first, then answers a reserved key before anything is applied: a
  `get` is the broker's entry of that name, or no entry when that side is
  not there or the key is unknown, with the store's `client` and
  `revision`; a `put` or a `delete` a refusal. A `list` adds the broker's
  entries to the stored ones, in one key order; a service nobody holds
  lists the broker's entries alone. `Store` is untouched.
- `BrokerHandler`: nothing; the refusal is counted as `refused` by the
  existing arm.
- Sizes: the broker's entries at their largest (IPv6 everywhere, 20-digit
  ids and key, `https://`) must fit `REPLY_OVERHEAD`, which grows from 1024
  to 4096 bytes so that `listen`'s check (budget plus overhead within the
  frame limit) still bounds a `list` of a full store; a unit test pins it,
  and the numbers in the README, the guides and the help text follow.
- Tests: types (shape, round trip, reserved keys), registry (a client, a
  service, an orphan, an unclaimed service, after a re-pairing; the
  refusals; `list`), end to end over every transport through both parties
  and across a re-pairing, `tests/cli.rs` (`nsm store get $HB nsm_mesh_data`
  prints the JSON, a `put` of it is exit 1), REST (one read).
- Docs: `docs/PROTOCOL.md` (types; the store section: reserved keys and the
  virtual entry; the refusal table), `docs/ARCHITECTURE.md` (the registry;
  the security model: a party's listener now also tells where its peer
  listens), `docs/REST_API.md`, README (the store section), `CHANGELOG.md`.

### `discover/03-store-by-key`

- `Message::StoreByKey { rendezvous, party_id, op }`: `kind`, `is_reply`,
  `all_variants`, the shape tests, the codec's random generator.
- `Registry::resolve_key(key, party_id) -> Result<PartyId>` with the rules
  of L5, and `Registry::store_by_key`.
- `BrokerHandler`: the arm, `RequestKind::StoreByKey` (`store_by_key`),
  store operations counted as for a relay.
- `PartyHandler`: nothing (unexpected, hence a nack); one test says so.
- `ops::StoreTarget { Party(Addr), Key { broker, key, party } }` with
  `StoreTarget::store`, and `ops::store_by_key` beside `ops::store`.
- CLI: `--key` and `--party-id` (which requires `--key`) on every `store`
  subcommand; the `<ADDR>` and `<STORE_KEY>` value names;
  `StoreCommand::into_parts` returns the target.
- REST: `StoreBody` with `party`, or `broker` and `rendezvous` (and
  `party_id`); the 400 texts for neither and for both.
- Tests: message, registry (the resolution table), handler (the counter),
  ops, CLI parsing; `tests/cli.rs` runs the lazy session: no `--bind-port`,
  discovery through the broker, `send` and `collect` with the discovered
  addresses, `put` and `get` by key, an ambiguous key refused and resolved
  with `--party-id`; end to end by key over every transport, an unknown key,
  an ambiguous one, a spare service beside the claim; REST with both forms,
  and 400 for neither and for both.
- Docs: `docs/PROTOCOL.md` (the message table, a section, the refusals, a
  sequence), `docs/REST_API.md`, `docs/ARCHITECTURE.md` (components, the
  security model, the paragraph on what the protocol listener reveals),
  `docs/MONITORING.md` (the `kind` label), README (a quickstart without
  chosen ports, the command table, the store section, a recipe),
  `CHANGELOG.md`.

## 4. Out of scope

- A port range to scan (`--bind-port-range LO-HI`, L1), for firewalls that
  open a fixed range.
- `send`, `collect` and `peer` by key (L10): the broker forwarding to the
  party, or answering `peer` from the registry.
- Reads only by key, or a broker flag that refuses `store_by_key` (L6).
- Answering several claims of one key in one reply (L5).
- A field selector on `nsm store get` (`--field nsm_mesh_client`); `jq`, or
  the shell patterns the README shows, do that.
- Discovering `--service-port`: the data-plane port is the application's,
  not nsm's.
- Mesh data on heartbeats or in `collect`: nothing is pushed (D20).

# Wire protocol

Protocol version **3** (`nsm::protocol::PROTOCOL_VERSION`). Everything below is
implemented in `src/protocol/` and `src/transport/`; the rustdoc of
`nsm::protocol::Message` is the authoritative field-by-field reference.
Version 3 let text flow both ways (`deliver` names no target); version 2 added
the answering party's `role` to `collected`; version 1 was the first versioned
format. Version 3 also carries the shared store's messages (`store`,
`store_relay`, `stored`), added compatibly: they are new variants, which by
the compatibility rule in section 6 need no bump. The conditional writes that
came after them (`if_version` on a put or a delete, `applied` on `stored`)
are optional fields that decode when absent, so version 3 carries them too.

## 1. Transports and framing

Every exchange is **one request followed by exactly one reply**, on every
transport. The same JSON message is carried in every case.

| Transport | Address | Wire |
|---|---|---|
| raw TCP | `host:port` (or `tcp://host:port`) | one length-prefixed frame each way, then the connection is closed |
| TCP + TLS | `tls://host:port` | as TCP, inside a TLS 1.2/1.3 session; no ALPN |
| HTTP | `http://host:port` | `POST /v1/message`, JSON body in, JSON body out; `GET /healthz` returns `{"ok":true}` |
| HTTPS | `https://host:port` | as HTTP, over TLS with ALPN `http/1.1`; the client refuses to fall back to plaintext |

**Frame format (TCP, TLS).** A 4-byte big-endian unsigned length, then that
many bytes of JSON. The length must not exceed the receiver's frame limit
(default 65536 bytes, `--max-frame-bytes`); a larger prefix closes the
connection before the body is read. A frame whose body is not a valid message
also closes the connection. A connection that stays silent, or a request that
takes longer than the request timeout (default 6 s), is dropped.

**HTTP.** Requests are `POST /v1/message` with a JSON body; the reply body is
the JSON reply message with status 200. A malformed body is answered with
status 400 and a `nack` body; a body larger than the frame limit with 413; a
handler failure with a `nack`. HTTP/1.1 only; connections may be kept alive.

**Envelope.** A message is a JSON object whose `type` field is the snake_case
variant name, followed by the variant's fields:

```json
{"type":"publish","key":1234,"service_port":9000,"bind_addr":"10.0.0.5:12010","ping":false}
```

Variants without fields are just the tag (`{"type":"collect"}`). An unknown
tag, a missing field or a field of the wrong type is a decode error; unknown
fields are ignored so that a newer peer may add some. `Option` fields are sent
as `null` when absent and may be omitted when decoding.

## 2. Types

| Type | JSON | Notes |
|---|---|---|
| `Key` | unsigned integer (u64) | the rendezvous key shared by a service and its clients |
| `PartyId` | unsigned integer (u64) | broker-assigned, sequential from 1, never reused within one broker process; not a secret |
| `RegToken` | string of 32 lowercase hex digits | 128 random bits issued at registration; compared in constant time; never logged |
| `Addr` | string | `host:port`, `tls://host:port`, `http://host:port` or `https://host:port`; IPv6 literals bracketed and canonicalised (`[::1]:80`) |
| `ServiceHandle` | `{"id":1,"host":"10.0.0.5","service_port":9000}` | what a client is told about its service; never carries the key |
| `Role` | `"service"` or `"client"` | which kind of party answered a `collect`, hence which field of `collected` applies |
| `StoreKey` | string | the name of one entry in a shared store: 1 to 128 characters from `A-Z a-z 0-9 . _ - : /`, not starting with `-`; anything else is a decode error. A store key is unrelated to the rendezvous key (`Key`) |
| store operation | `{"op":"get","key":"step"}`, `{"op":"put","key":"step","value":"5","if_version":null}`, `{"op":"delete","key":"step","if_version":null}`, `{"op":"list"}` | carried inside `store` and `store_relay`, its fields next to `type`; a value is any UTF-8 text, empty text included; `if_version` (an unsigned integer or `null`, which is also what a missing field means) makes a put or a delete conditional |
| `StoreEntry` | `{"key":"step","value":"5","version":3}` | one entry of a store; `version` is the number of its last write |
| `MeshData` | `{"nsm_key":1234,"nsm_service_id":1,"nsm_service_address":"10.0.0.5","nsm_service_port":9000,"nsm_service":"10.0.0.5:9000","nsm_mesh_service_address":"10.0.0.5","nsm_mesh_service_port":12010,"nsm_mesh_service":"10.0.0.5:12010","nsm_client_id":2,"nsm_mesh_client_address":"10.0.0.6","nsm_mesh_client_port":12020,"nsm_mesh_client":"10.0.0.6:12020"}` | where the parties of a claim listen: the value of the reserved store entry `nsm_mesh_data`, as JSON text, built by the broker from its registry when the entry is read; the `*_address` fields are hosts (IPv6 without brackets), the `*_port` fields ports, and `nsm_service`, `nsm_mesh_service` and `nsm_mesh_client` the same three endpoints as one string each (the data-plane endpoint as `host:port`, the heartbeat addresses with their transport); a side that is not there is `null`; every field that is set is also a reserved entry of its own under the field's name, carrying the field's text |

## 3. Messages

| Request | Direction | Reply |
|---|---|---|
| `publish` | service → broker | `registered` or `nack` |
| `claim` | client → broker | `paired` or `nack` |
| `ping` | ping-mode party → broker | `heartbeat` or `nack` |
| `send` | operator → party | `delivered` or `nack` |
| `deliver` | party → broker (relay of a `send`) | `delivered` or `nack` |
| `heartbeat` | broker → party (two-sided liveness) | `heartbeat_ack` or `nack` |
| `collect` | operator → party | `collected` |
| `store` | operator → party | `stored` or `nack` |
| `store_relay` | party → broker (relay of a `store`) | `stored` or `nack` |

### `publish` → `registered`

A service registers under `key`. Its data-plane endpoint is `bind_addr.host`
together with `service_port`; `bind_addr` itself is where the broker
heartbeats it and names the transport the service listens with. With
`ping: true` the service will ping instead of being dialled.

```json
{"type":"publish","key":1234,"service_port":9000,"bind_addr":"10.0.0.5:12010","ping":false}
{"type":"registered","id":1,"token":"3f9c0a7b1d2e4f60a1b2c3d4e5f60718"}
```

Refused (`nack`) when admission fails: `bind_addr` has port 0, the advertised
host differs from the connection source while `--require-matching-host` is
on, the per-host cap is reached, or the broker holds `--max-registrations`
parties already.

### `claim` → `paired`

A client asks for a service under `key`. `bind_addr` and `ping` mean the same
as for `publish`.

```json
{"type":"claim","key":1234,"bind_addr":"http://10.0.0.6:12020","ping":true}
{"type":"paired","id":2,"token":"9e8d7c6b5a4f30211f2e3d4c5b6a7980","service":{"id":1,"host":"10.0.0.5","service_port":9000}}
```

The broker pairs the client with the **lowest-id unclaimed** service of that
key, exclusively. When none is free it waits up to 1.5 s (`claim_wait`) for
one to appear, then answers `{"type":"nack","reason":"no service available for key 1234"}`.
Admission rules are the same as for `publish`.

### `ping` → `heartbeat`

One-sided liveness. A party registered with `ping: true` sends this every
heartbeat interval, quoting its id and token.

```json
{"type":"ping","id":2,"token":"9e8d7c6b5a4f30211f2e3d4c5b6a7980"}
{"type":"heartbeat","token":"9e8d7c6b5a4f30211f2e3d4c5b6a7980","inbox":null,"service":null}
```

The reply is exactly the heartbeat the broker would have sent in two-sided
mode, so both modes deliver the same things. Refused with one text,
`unknown party or wrong token`, for an unknown id and for a wrong token
(ids cannot be enumerated); `party is not in ping mode` for a two-sided
party. A party that receives a `nack` treats its registration as lost.

### `heartbeat` → `heartbeat_ack`

Two-sided liveness. The broker dials the party's `bind_addr` every heartbeat
interval. `inbox` is text pending for the party; `service` is a client's new
pairing after a re-pairing. Each pending item is delivered once; if the
heartbeat fails, the item is restored and carried by the next one.

```json
{"type":"heartbeat","token":"3f9c0a7b1d2e4f60a1b2c3d4e5f60718","inbox":"job 17","service":null}
{"type":"heartbeat_ack","id":1}
```

A party applies a heartbeat only when `token` is its own; otherwise it answers
`{"type":"nack","reason":"heartbeat does not carry this party's token"}` and
changes nothing, so nobody but the broker can deliver text, re-pair a client
or keep its watchdog quiet. A service ignores the `service` field. The broker
treats any acknowledgement as proof of life and only logs an id mismatch.

### `send` → `delivered`, `deliver` → `delivered`

`send` is what the `nsm send` operation sends to a party's bind address, of
either kind; the party relays it to the broker as `deliver`, naming itself and
its token, and passes the broker's answer back.

```json
{"type":"send","text":"job 17"}
{"type":"deliver","from":2,"token":"9e8d7c6b5a4f30211f2e3d4c5b6a7980","text":"job 17"}
{"type":"delivered"}
```

`deliver` names no target: the broker delivers to the sender's peer as it
knows it, a client's current service or the client holding a service, so a
text that races a re-pairing reaches the new service instead of being
refused. The broker accepts a `deliver` only from a registered party
presenting its token, and only while that party has a peer: `service <id> is
not claimed` for a service nobody holds, `client <id> has no service` for a
client whose service died and that has not been re-paired yet. It parks the
text as the peer's pending inbox; a later `deliver` before the hand-over
replaces it. A `send` to a party that has not registered yet is refused by
the party itself.

### `collect` → `collected`

```json
{"type":"collect"}
{"type":"collected","role":"service","text":"job 17","service":null}
{"type":"collected","role":"client","text":"ready","service":{"id":1,"host":"10.0.0.5","service_port":9000}}
```

The reply names the party's `role`, and the role says which fields apply.
Both kinds answer with the last text they received (`text`, `null` if none
yet); a client also answers with the handle of the service it is paired with
(`service`, `null` only in the moment between binding its listener and
registering), which a service leaves empty. The asker never has to guess from
which field is set.

### `store` → `stored`, `store_relay` → `stored`

Each claim has one small key-value store, kept by the broker and shared by
the client and the service that holds the claim. A store operation travels
from the operator to a party and from the party to the broker: `store` is
the operator's request to either party's bind address; the party keeps no
copy and relays it to the broker as `store_relay`, naming itself and its
token, the way `send` becomes `deliver`, and passes the broker's answer back
to the operator unchanged. The operator never sees a token. The operation's
fields sit next to `type`:

```json
{"type":"store","op":"put","key":"step","value":"5","if_version":null}
{"type":"store","op":"get","key":"step"}
{"type":"store","op":"delete","key":"step","if_version":null}
{"type":"store","op":"list"}
{"type":"store_relay","from":2,"token":"9e8d7c6b5a4f30211f2e3d4c5b6a7980","op":"put","key":"step","value":"5","if_version":null}
```

The reply to both is `stored`: `client` is the id of the client whose claim
owns the store, `revision` the number of the store's last write (0 before the
first), `applied` whether a write went through (false only for a conditional
write whose condition did not hold, see below), and `entries` what the
operation returns.

```json
{"type":"stored","client":2,"revision":3,"applied":true,"entries":[{"key":"step","value":"5","version":3}]}
{"type":"stored","client":2,"revision":3,"applied":true,"entries":[]}
{"type":"stored","client":null,"revision":0,"applied":true,"entries":[]}
```

| Operation | `entries` |
|---|---|
| `get` | the entry, or none when the key is not set |
| `put` | the entry as written, with its new version |
| `delete` | the removed entry, with the version of its last write, or none when the key was not set (which is not an error) |
| `list` | every entry, in ascending byte order of key: one consistent snapshot of the stored entries, with the broker's reserved entries (below) among them at version 0 |

`store_relay` names no store: the broker finds it from `from`, as it finds
the peer for `deliver`. A client uses its own claim's store, also between
losing its service and being re-paired. A service uses the store of the
client holding it; a service nobody holds reads an empty store
(`client: null`, the last example above) and may not write. Every write (a
put, or a delete that removed an entry) takes the next number from one
counter for the broker's whole life, so versions never repeat, not even
across claims, and one store's versions may skip numbers other stores took.
Reads, refusals and deletes of an absent key take no number. The token check
and the operation run in one critical section, so each operation is atomic
against every other operation and against re-pairings; the last writer wins
per key, unless the writer states a condition.

**Reserved keys.** Store keys starting with `nsm_` are the broker's: never
stored, never written, counted against no budget, but projected from the
broker's registry at the moment they are read, so they are always current,
across re-pairings too. They say where the parties of the claim listen.
`nsm_mesh_data` is the whole picture as one JSON value, a `MeshData` (see
the types table); every field of it that is set is also an entry of its own
under the field's name, carrying the field's text, so a shell script needs
no JSON parser: `nsm_service_address` and `nsm_service_port` (the service's
data-plane endpoint, also as `nsm_service`), `nsm_mesh_service_address` and
`nsm_mesh_service_port` (where the service listens for heartbeats, also as
`nsm_mesh_service`, with its transport), `nsm_mesh_client_address` and
`nsm_mesh_client_port` (where the client does, also as `nsm_mesh_client`),
`nsm_key`, `nsm_service_id` and `nsm_client_id`. A field that is `null` has
no entry: at a service nobody holds, a `get` of `nsm_mesh_client` answers no
entry, like a key that is not set, until a client claims; at a client whose
service died, the service's keys are missing until the re-pairing. Every
reserved entry has `version` 0, since nothing was written; `client` and
`revision` are the store's, as for any read (`client: null` at a service
nobody holds). A `list` carries them all beside the stored entries, in one
key order, so a store never lists as empty: `nsm_key` and `nsm_mesh_data`
are always there. A `put` or a `delete` of any `nsm_` key is refused with
`<key> is reserved: store keys starting with nsm_ are the broker's`,
whatever its condition and whoever asks, and a `get` of an `nsm_` key the
broker does not know answers no entry. The reserved entries at their
largest fit within the 4096 bytes `listen` adds to the budget when it
checks the frame limit (see the sizes rule).

```json
{"type":"store","op":"get","key":"nsm_service_port"}
{"type":"stored","client":2,"revision":3,"applied":true,"entries":[{"key":"nsm_service_port","value":"9000","version":0}]}
{"type":"store","op":"get","key":"nsm_mesh_data"}
{"type":"stored","client":2,"revision":3,"applied":true,"entries":[{"key":"nsm_mesh_data","value":"{\"nsm_key\":1234,\"nsm_service_id\":1,...}","version":0}]}
```

**Conditional writes.** A put or a delete may carry `if_version`, compared
with the key's current version in the same critical section that applies
the write. Versions start at 1, so 0 stands for "not set":

| `if_version` | The write is applied when |
|---|---|
| `null` or missing | always (the last writer wins) |
| `0` | the key is not set; a put creates it, a delete removes nothing |
| `n` > 0 | the key is set and its current version is `n` |

When the condition does not hold, the answer is still `stored`, not a
`nack`: `applied` is `false`, `entries` carries the key's current entry (or
none when it is not set), `revision` is the store's revision as it was, and
nothing changes; no version number is taken. The condition is checked before
the budget, so a stale write is answered this way even when its value would
not have fitted. A service nobody holds is refused its writes with
`service <id> is not claimed` whatever the condition.

```json
{"type":"store","op":"put","key":"step","value":"8","if_version":7}
{"type":"stored","client":2,"revision":9,"applied":false,"entries":[{"key":"step","value":"7","version":9}]}
{"type":"store","op":"put","key":"task","value":"mine","if_version":0}
{"type":"stored","client":2,"revision":10,"applied":true,"entries":[{"key":"task","value":"mine","version":10}]}
```

Of two writers that read the same version and both write with it as their
condition, exactly one is applied; the other learns the winner's entry and
can retry from it. That is how two parties update one key without losing
each other's changes.

The broker refuses a `store_relay` with a `nack`, changing nothing:

| Reason | When |
|---|---|
| `unknown party or wrong token` | the id is unknown or removed, or the token is not its own (one text for both) |
| `<key> is reserved: store keys starting with nsm_ are the broker's` | a put or a delete named a reserved key, from either party, with or without a condition |
| `service <id> is not claimed` | a service nobody holds tried to put or delete |
| `store full: the entry needs N bytes and F of M are free` | the put does not fit the store's budget (see the sizes rule) |
| `store versions exhausted` | the version counter ran out, after about 1.8 × 10^19 writes |

The party adds refusals of its own:

| Reason | When |
|---|---|
| `service is not registered yet`, `client is not registered yet` | the party has bound its listener but not registered; it answers without asking the broker, as it does a `send` |
| `broker answered a store relay with <kind>` | the broker's reply was neither `stored` nor `nack` |

When the broker cannot be reached, the party fails the request as it fails a
`send`: over TCP and TLS it closes the connection, over HTTP and HTTPS it
answers status 500 with a `nack`. A `store` sent to the broker is
`unexpected store at the broker`, since it carries no credentials, and a
`store_relay` sent to a party is `unexpected store_relay at a service` (or
`client`). A malformed operation (an invalid store key, an unknown `op`, a
put without a value) is a decode error like any other malformed message.

### `nack`

```json
{"type":"nack","reason":"no service available for key 1234"}
```

The request was understood but cannot be honoured. Malformed input is never
answered with a `nack`: the transport closes the connection or returns an HTTP
error instead.

## 4. Sequences

### Registration and pairing

```text
service                    broker                    client
   │ bind :12010              │                         │
   │── publish(key, 9000) ───►│                         │
   │◄─ registered(id 1, T1) ──│                         │
   │                          │◄── claim(key) ──────────│ bind :12020
   │                          │── paired(id 2, T2, ────►│
   │                          │        {1, host, 9000}) │ prints host:9000
   │◄── heartbeat(T1) ────────│──── heartbeat(T2) ─────►│   every 2 s
   │─── heartbeat_ack(1) ────►│◄─── heartbeat_ack(2) ───│
```

### Text delivery

```text
operator            client (2)              broker              service (1)
   │── send("x") ─────►│                       │                    │
   │                   │── deliver(2,T2,"x") ─►│ inbox[1] = "x"     │
   │                   │◄── delivered ─────────│                    │
   │◄── delivered ─────│                       │── heartbeat(T1, ──►│ inbox = "x"
   │                   │                       │      inbox "x")    │
   │────────────────────── collect ─────────────────────────────────►│
   │◄───────────────────── collected(text "x") ─────────────────────│
```

The other direction is the same picture mirrored: `send` to the service's
bind address, `deliver(1,T1,"y")` from the service, `inbox[2] = "y"` at the
broker, the text carried by the client's next heartbeat, and `collect` at the
client.

### Ping mode

```text
party (behind NAT)                        broker
   │── ping(id, T) ──────────────────────────►│ mark alive
   │◄─ heartbeat(T, inbox?, service?) ────────│ deliver pending items
   │   ... every heartbeat interval ...       │
   │   (silent for ping_staleness)            │ removed by the sweeper
```

### A service dies

```text
service A (1)      broker                         client (2)           service B (3)
   ✕                 │── heartbeat ──✕  fail 1        │                     │
                     │── heartbeat ──✕  fail 2 ...    │                     │
                     │   fail_threshold reached:      │                     │
                     │   remove 1; reclaim(2) → 3     │                     │
                     │── heartbeat(T2, service {3}) ─►│ new service address │
                     │◄─ heartbeat_ack(2) ────────────│                     │
                     │   (no service B: remove 2; its watchdog fires, exit 1)
```

### The shared store

```text
operator            client (2)                broker                      service (1)
   │── store ──────────►│                        │                            │
   │   put step=5       │── store_relay(2,T2, ──►│ claim 2's store:           │
   │                    │     put step=5)        │   step=5, version 7        │
   │                    │◄─ stored(client 2, ────│                            │
   │◄─ stored ──────────│     rev 7, step=5 v7)  │                            │
   │─────────────────────────── store get step ──────────────────────────────►│
   │                    │                        │◄─ store_relay(1,T1, ───────│
   │                    │                        │      get step)             │
   │                    │                        │── stored(client 2, ───────►│
   │                    │                        │      rev 7, step=5 v7)     │
   │◄────────────────────────── stored ───────────────────────────────────────│
```

Neither party keeps anything: each request is answered from the one copy at
the broker, in the reply itself, so nothing rides on heartbeats and the
second read needs no hand-over. The version is 7 rather than 1 because every
store takes its numbers from one broker-wide counter. A write acknowledged
before a `send` is visible to the peer by the time the peer has the text: the
write was applied before the `deliver` reached the broker.

### A store across a re-pairing

```text
service A (1)        broker                              client (2)          service B (3)
   │── store_relay ───►│ put a=x (version 4)                  │                    │
   │   (1,T1, put a=x) │                                      │                    │
   ✕                   │◄─── store_relay(2,T2, put c=y) ──────│                    │
                       │     version 5                        │                    │
                       │   fail_threshold reached:            │                    │
                       │   remove 1; reclaim(2) → 3           │                    │
                       │── heartbeat(T2, service {3}) ───────►│                    │
                       │◄─────────────── store_relay(3,T3, list) ──────────────────│
                       │──────────────── stored(client 2, rev 5, a=x v4, c=y v5) ─►│
                       │◄─────────────── store_relay(3,T3, put b=z) ───────────────│
                       │──────────────── stored(client 2, rev 6, b=z v6) ─────────►│
```

The store belongs to the claim, not to the pairing: the client may use it
while it has no service, the replacement reads everything written before
(the dead service's writes included) under the same `client`, and its own
writes continue the numbering. Before `reclaim` gives it the claim, B is an
unclaimed service like any other: it reads an empty store with
`client: null` and may not write. A's token stops verifying the moment A is
removed, so a stale A can no longer write. When the client itself is
removed, its store goes with it, and the next claim of the freed service
starts with an empty store under a new `client`.

## 5. Timing

All values are configurable (`--heartbeat-interval` and friends); broker and
parties should agree on them.

| Parameter | Default | Used by |
|---|---|---|
| heartbeat interval | 2 s | broker (two-sided heartbeats, sweeper period), party (ping period) |
| heartbeat timeout | 3 s | broker: one heartbeat exchange |
| fail threshold | 5 | broker: consecutive failures before removal; party: consecutive failed pings before giving up |
| ping staleness | 20 s | broker: silence before a ping-mode party is removed |
| broker watchdog | 30 s | two-sided party: silence before it gives up on the broker |
| request timeout | 6 s | every request/response exchange |
| connect timeout | 5 s | connecting and the TLS handshake |
| registration | 6 attempts, 1 s apart | party: retried on connection failures only; a `nack` is final |
| claim wait | 1.5 s | broker: how long a claim waits for a service to appear |

A two-sided party is detected dead within `fail_threshold × (interval +
timeout)`, about 25 s with the defaults; a service's text is delivered within
one interval; a re-paired client learns its new service within one interval.

## 6. Rules

- **Exclusive claims.** A service is held by at most one client, from the
  claim until that client is removed. There is no time-based lease.
- **Re-pairing.** When a service is removed, each of its clients is paired
  with the lowest-id unclaimed service of the same key and told in its next
  heartbeat (`service`). A client with no replacement is removed.
- **Freeing.** When a client is removed, its service becomes unclaimed and can
  be claimed again.
- **Ids** are never reused within a broker process; a removed id stays
  unknown.
- **Stores follow the claim.** A claim's store is created empty when the
  claim is granted, kept when the client is re-paired (the replacement
  service reads everything written before, the dead service's writes
  included) and dropped when the client is removed; the next claim of the
  freed service starts with an empty store under a new client id. A service
  reaches the store only while it holds the claim. Nothing store-related
  rides on heartbeats or ping replies, a store relay does not count as proof
  of life, and nothing survives a broker restart. Store keys starting with
  `nsm_` are the broker's: the entries of `nsm_mesh_data` are answered from
  the registry, listed at version 0, never stored and never written.
- **Tokens** are required on `ping`, `deliver`, `store_relay` and `heartbeat`; the broker's
  refusals do not reveal whether an id exists. Tokens are 16 random bytes from
  the TLS crypto provider's secure random source.
- **Sizes.** Every message is limited to the frame limit; text in `send`,
  `deliver`, `heartbeat` and `collected` is carried verbatim inside the JSON
  string and shares that limit. A store has a budget of its own
  (`--max-store-bytes` on `listen`, default 16384, allowed 256 to 32768):
  each entry counts as its key and value encoded as JSON strings, quotes and
  escapes included, plus 64 bytes, and a put that would exceed the budget is
  refused. A `stored` reply carrying every entry of a full store, the
  broker's reserved entries included, is then at most the budget plus 4096
  bytes, and `listen` refuses to start unless that fits its own
  `--max-frame-bytes`, which a broker therefore needs at 4352 or more
  (20480 with the default budget). Parties use the default frame limit of
  65536, which every allowed budget fits. `nsm serve` uses its
  `--max-frame-bytes` for the parties it starts and for every reply it reads
  itself, whichever way the party was started, so lowering it below a
  store's reply size breaks large replies there: at those parties, and as a
  400 (oversized frame) from `POST /v1/store`.
- **Compatibility.** `PROTOCOL_VERSION` is bumped for any change an older
  peer could not decode: a renamed or removed field or variant, a changed
  framing. Adding an optional field or a new variant does not require a bump;
  that is how version 3 came to carry the store messages and then their
  conditions. There is no negotiation; brokers and parties must run the same
  version. A broker that predates the store cannot decode a `store_relay`:
  over TCP and TLS it closes the connection, over HTTP and HTTPS it answers
  status 400 with a `nack` (see section 1). A broker that has the store but
  predates conditional writes ignores `if_version` like any unknown field and
  applies the write unconditionally; its `stored` has no `applied`, which
  decodes as true. A party that has the store but predates conditional
  writes does the same on the way through: it decodes `store` without
  `if_version`, relays it without one, and passes the broker's `stored` on
  without `applied`. Either way a conditional write becomes an unconditional
  one that is answered as applied, and nothing reports it, so conditions
  need both the broker and the party that relays them to have conditional
  writes. Parties run for as long as their jobs do, so restart them on the
  new binary before relying on `if_version`. A broker that predates the
  reserved keys treats `nsm_mesh_data` as an ordinary key: a `get` answers
  "not set" (or whatever a `put` wrote there), and nothing is refused.

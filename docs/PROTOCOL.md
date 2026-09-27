# Wire protocol

Protocol version **3** (`nsm::protocol::PROTOCOL_VERSION`). Everything below is
implemented in `src/protocol/` and `src/transport/`; the rustdoc of
`nsm::protocol::Message` is the authoritative field-by-field reference.
Version 3 let text flow both ways (`deliver` names no target); version 2 added
the answering party's `role` to `collected`; version 1 was the first versioned
format.

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
- **Tokens** are required on `ping`, `deliver` and `heartbeat`; the broker's
  refusals do not reveal whether an id exists. Tokens are 16 random bytes from
  the TLS crypto provider's secure random source.
- **Sizes.** Every message is limited to the frame limit; text in `send`,
  `deliver`, `heartbeat` and `collected` is carried verbatim inside the JSON
  string and shares that limit.
- **Compatibility.** `PROTOCOL_VERSION` is bumped for any change an older
  peer could not decode: a renamed or removed field or variant, a changed
  framing. Adding an optional field or a new variant does not require a bump.
  There is no negotiation; brokers and parties must run the same version.

# REST control plane

`nsm serve` exposes the operations over HTTP so an orchestrator can drive NSM
without a shell. It is the same code path as the CLI (`nsm::ops`); the request
bodies are the CLI's option structs serialised as JSON.

```bash
nsm serve                                  # 127.0.0.1:8080, no token needed
nsm serve --bind 0.0.0.0:8080 --token "$NSM_TOKEN"   # any other address needs a token
```

| Option | Default | Meaning |
|---|---|---|
| `--bind ADDR` | `127.0.0.1:8080` | address to listen on |
| `--token TOKEN` (`NSM_TOKEN`) | none | bearer token; **required** unless `--bind` is a loopback address |
| TLS, timing and limit options | as the CLI | handed to every party this server starts (`--tls` itself has no effect; a job asks for TLS in its body) |

## Conventions

- Requests and responses are JSON. Request bodies may be sent without a
  `Content-Type`; they are limited to 64 KiB (413 above that).
- With a token configured, every request, including `GET /healthz`, must carry
  `Authorization: Bearer <token>`; anything else is 401
  `{"error":"missing or invalid bearer token"}`. The comparison is constant
  time.
- Errors are `{"error":"<message>"}`:

| Status | When |
|---|---|
| 400 | malformed or incomplete body, bad address or query value, a refusal by the broker or a party (unknown key, admission, a store write at a service nobody holds, a full store), oversized frame |
| 401 | missing or wrong bearer token |
| 404 | unknown job id, unknown route |
| 405 | wrong method on a known route |
| 409 | a store put or delete whose `if_version` did not match; the body is the store's reply with an `error` field added (see [`POST /v1/store`](#post-v1store)) |
| 413 | body larger than 64 KiB |
| 502 | the broker or a party could not be reached (connection refused, timeout, connection closed, name resolution) |
| 500 | anything else (for example the party's own listener could not be bound) |

- Long-running operations are **jobs**: `POST /v1/publish` and
  `POST /v1/claim` register the party synchronously (so a refusal is reported
  as a 400 with the reason, not as a job that later fails), then keep its
  session running in the background and answer `202 Accepted` with a job
  view. Jobs end when they are cancelled, when the server stops, or when the
  session fails (broker lost, no replacement service); the view keeps the
  final state.

## Routes

### `GET /healthz`

```json
{"ok":true}
```

### `GET /v1/interfaces?ip_version=4`

Interface names on this host. `ip_version` (`4` or `6`) is optional.

```json
{"interfaces":["lo","eth0","hsn0"]}
```

### `GET /v1/ips?interface=eth0&ip_start=10.128.&ip_version=4`

Addresses on this host with the CLI's filters (`interface` is `-n`, `ip_start`
is `-i`); all optional. A filter that matches nothing is an empty list.

```json
{"addresses":[{"interface":"eth0","ip":"10.128.0.7"}]}
```

### `POST /v1/publish`

Start a service party.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `broker` | address string | yes | the broker (`host:port`, `tls://`, `http://`, `https://`) |
| `key` | integer | yes | rendezvous key |
| `service_port` | integer | yes | port the real service listens on |
| `bind_port` | integer | no (0) | heartbeat port; 0 or omitted lets the operating system pick a free one, as leaving `--bind-port` out does on the command line |
| `interface`, `ip_start`, `ip_version` | strings | no | local address selection, as `-n`, `-i`, `--ip-version` |
| `tls` | boolean | no (false) | serve TLS on the heartbeat listener, using the server's certificate |
| `ping` | boolean | no (false) | one-sided liveness |

```bash
curl -s -X POST http://127.0.0.1:8080/v1/publish -H 'content-type: application/json' \
  -d '{"broker":"http://10.0.0.1:12000","key":1234,"service_port":9000,"interface":"hsn0","ip_version":"4"}'
```

```json
{"id":1,"kind":"publish","state":"running","error":null,"party_id":7,
 "bind_addr":"http://10.128.0.7:41231","service":null,"broker":"http://10.0.0.1:12000","key":1234}
```

Status 202. A broker refusal (for example the per-host cap) is a 400 with the
broker's reason; an unreachable broker is a 502.

### `POST /v1/claim`

Start a client party. Same fields as `publish` without `service_port`.

```json
{"id":2,"kind":"claim","state":"running","error":null,"party_id":8,
 "bind_addr":"http://10.128.0.9:41232","service":{"id":7,"host":"10.128.0.7","service_port":9000},
 "broker":"http://10.0.0.1:12000","key":1234}
```

`service` is the paired service; it is updated when the broker re-pairs the
client, so poll `GET /v1/jobs/{id}` to follow it. A key with no service is a
400 `{"error":"broker rejected the request: no service available for key 1234"}`.

### `GET /v1/jobs`, `GET /v1/jobs/{id}`

All job views (oldest first), or one. Unknown ids are 404; a non-numeric id is
400.

### `DELETE /v1/jobs/{id}`

Stops the job's session: the party's listener closes and the broker removes
the party once its heartbeats fail. Returns the view with `state` set to
`cancelled`; the job stays listed. 404 for unknown ids.

### `POST /v1/collect`

```json
{"party":"http://10.128.0.7:41231"}
```

```json
{"role":"service","text":"job 17"}
{"role":"client","service":{"id":7,"host":"10.128.0.7","service_port":9000},"text":"ready"}
```

`role` names the kind of party that answered and therefore the other fields:
`text` is the last text the party received from its peer (`null` if none
yet); for a client, `service` is also its paired service (`null` only before
it has registered). An unreachable party is a 502.

### `POST /v1/send`

```json
{"party":"http://10.128.0.9:41232","msg":"job 17"}
```

```json
{"delivered":true}
```

`party` is either party's heartbeat address; the party relays the text
through the broker to its peer (a client's service, or the client holding a
service), which receives it on its next heartbeat. A service that no client
holds is a 400 with the reason.

### `POST /v1/store`

One operation on the store a client shares with the service it holds,
through either party. The operation's fields sit next to `party`, in the
shape of the protocol's `store` message.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `party` | address string | yes | either party's heartbeat address (a job's `bind_addr`); the client and its service reach the same store |
| `op` | string | yes | `"get"`, `"put"`, `"delete"` or `"list"` |
| `key` | string | for get, put, delete | store key: 1 to 128 characters from `A-Z a-z 0-9 . _ - : /`, not starting with `-` (unrelated to the rendezvous `key` of publish and claim) |
| `value` | string | for put | the new value: any text, including empty text and newlines |
| `if_version` | unsigned integer or `null` | no | for put and delete: apply the write only if the key's current version is this number, or only if the key is not set when it is 0; `null` or missing writes unconditionally |

```json
{"party":"http://10.128.0.9:41232","op":"put","key":"step","value":"5"}
{"party":"http://10.128.0.9:41232","op":"put","key":"step","value":"6","if_version":3}
{"party":"http://10.128.0.7:41231","op":"get","key":"step"}
{"party":"http://10.128.0.7:41231","op":"delete","key":"step"}
{"party":"http://10.128.0.7:41231","op":"list"}
```

Every operation answers 200 with the broker's reply, the same line
`nsm store ... --json` prints, except a conditional write that did not match
(409, below):

```json
{"client":8,"revision":3,"applied":true,"entries":[{"key":"step","value":"5","version":3}]}
```

`client` is the party id of the client whose claim owns the store, whichever
party was asked, and `null` when a service nobody holds reads its store.
`revision` is the number of the store's last write (0 for a store never
written). Versions come from one counter for the broker's whole life, so a
version names one write and is never issued twice. `entries` depends on the
operation:

| `op` | `entries` |
|---|---|
| `get` | the entry, or `[]` when the key is not set (still 200, as `collect` answers `text: null`) |
| `put` | the entry as written, with its new version |
| `delete` | the removed entry, or `[]` when the key was not set (still 200) |
| `list` | every entry, sorted by key: one consistent snapshot |

`applied` is true for every 200. A put or a delete with `if_version` is
checked by the broker in the same step as the write: of two writers that
read version 3 and both send `"if_version":3`, exactly one is applied. The
other is answered 409 with the reply, `applied` false and the key's current
entry (or `[]` when it is not set), plus an `error` field that says where
the key is. Nothing changed, so the caller can retry from that entry:

```json
{"error":"store key step is at version 7","client":8,"revision":7,"applied":false,"entries":[{"key":"step","value":"6","version":7}]}
{"error":"store key step is not set","client":8,"revision":9,"applied":false,"entries":[]}
```

`"if_version":0` creates a key only if it is not set, so of several
create-only puts of one key the first wins. A delete with 0 of a key that is
not set is a 200 that removes nothing.

Conditions need the broker and the party both to have conditional writes. A
broker or a party that predates them drops `if_version`, applies the write
anyway and answers 200 as if the condition held, so restart long-running
parties on the new binary before relying on it.

A service nobody holds reads an empty store (`{"client":null,"revision":0,"applied":true,"entries":[]}`)
and its put and delete are a 400 `service <id> is not claimed`, with or
without `if_version`. Other 400s:
an invalid or missing `key`, an unknown `op`, a put without `value`, an
`if_version` that is not an unsigned integer, a party
that has not registered yet, a put that does not fit the store's budget
(`store full: ...`), a party whose registration is gone, and a reply larger
than the server's own `--max-frame-bytes`. An unreachable party is a 502; for
a put or a delete that means the outcome is unknown, not that nothing
changed (the broker may have applied the write before the reply was lost),
so read the key to find out. The store is readable and writable by anyone
who can reach a party's heartbeat address; it is not a place for secrets.

## Job view

| Field | Meaning |
|---|---|
| `id` | job id, unique within this server process, starting at 1 |
| `kind` | `"publish"` or `"claim"` |
| `state` | `"running"`, `"failed"` (broker lost, or the party was removed), `"cancelled"` (after `DELETE`), `"finished"` (the server stopped) |
| `error` | why the job failed, when it did |
| `party_id` | the broker-assigned id |
| `bind_addr` | where the party listens for heartbeats (also what `collect`, `send` and `store` take) |
| `service` | for a claim: the paired service (updated on re-pairing) |
| `broker`, `key` | as requested |

Job views never contain the registration token or file paths.

## A complete session with curl

```bash
B=http://127.0.0.1:8080
curl -s $B/healthz
curl -s -X POST $B/v1/publish -d '{"broker":"http://127.0.0.1:12000","key":77,"service_port":9100,"ip_start":"127.","ip_version":"4"}'
curl -s -X POST $B/v1/claim   -d '{"broker":"http://127.0.0.1:12000","key":77,"ip_start":"127.","ip_version":"4"}'
curl -s -X POST $B/v1/send    -d '{"party":"<claim bind_addr>","msg":"job 17"}'
curl -s -X POST $B/v1/collect -d '{"party":"<publish bind_addr>"}'
curl -s -X POST $B/v1/store   -d '{"party":"<claim bind_addr>","op":"put","key":"step","value":"5"}'
curl -s -X POST $B/v1/store   -d '{"party":"<publish bind_addr>","op":"get","key":"step"}'
curl -s -X POST $B/v1/store   -d '{"party":"<publish bind_addr>","op":"list"}'
curl -s -X POST $B/v1/store   -d '{"party":"<publish bind_addr>","op":"put","key":"step","value":"6","if_version":1}'   # 409 unless step is at version 1
curl -s -X DELETE $B/v1/jobs/2
```

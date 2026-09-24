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
| 400 | malformed or incomplete body, bad address or query value, a refusal by the broker (unknown key, admission), oversized frame |
| 401 | missing or wrong bearer token |
| 404 | unknown job id, unknown route |
| 405 | wrong method on a known route |
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
| `bind_port` | integer | no (0) | heartbeat port; 0 picks a free one |
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
{"text":"job 17","service":null}
```

For a service, `text` is the last text it received (`null` if none yet); for a
client, `service` is its paired service. An unreachable party is a 502.

### `POST /v1/send`

```json
{"party":"http://10.128.0.9:41232","msg":"job 17"}
```

```json
{"delivered":true}
```

`party` is a **client's** heartbeat address; the client relays the text
through the broker to its service, which receives it on its next heartbeat. A
party that is a service, or a client that is not paired, is a 400 with the
reason.

## Job view

| Field | Meaning |
|---|---|
| `id` | job id, unique within this server process, starting at 1 |
| `kind` | `"publish"` or `"claim"` |
| `state` | `"running"`, `"failed"` (broker lost, or the party was removed), `"cancelled"` (after `DELETE`), `"finished"` (the server stopped) |
| `error` | why the job failed, when it did |
| `party_id` | the broker-assigned id |
| `bind_addr` | where the party listens for heartbeats (also what `collect` and `send` take) |
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
curl -s -X DELETE $B/v1/jobs/2
```

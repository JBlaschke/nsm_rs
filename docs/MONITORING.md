# Monitoring

A broker can report what it holds and what it has done: how many services
and clients are registered and under which keys, whether heartbeats are
failing, how many registrations it refused and why, how full the stores
are. Two front-ends read the same numbers: `GET /metrics` for Prometheus and
Grafana (the time series) and `GET /v1/status` for a shell (the moment).
Both are served by an **admin listener** that `nsm listen` starts only when
asked, so a broker started as before behaves as before. The design is
recorded in the [monitoring plan](PLAN.md).

## The admin listener

```bash
nsm listen --bind-port 12000 -i 127. --ip-version 4 --admin-bind 127.0.0.1:9108
```

| Option | Default | Meaning |
|---|---|---|
| `--admin-bind ADDR` | off | serve `GET /metrics`, `GET /v1/status` and `GET /healthz` on this address, over plain HTTP |
| `--admin-token TOKEN` (`NSM_ADMIN_TOKEN`) | none | bearer token; **required** unless `--admin-bind` is a loopback address |

The admin listener is a second socket next to the protocol listener. The
protocol listener may be raw TCP or TLS, and Prometheus scrapes HTTP, so a
separate listener is the one arrangement that works for every transport;
parties never talk to it, and nothing on it changes the broker's state. It
is off unless asked for, because a fixed default port would collide when
two brokers share a login node. The broker prints the address it bound on
stderr: `nsm: admin listener on http://127.0.0.1:9108`.

The exposure rules are the control plane's ([`REST_API.md`](REST_API.md)):
loopback needs no token; any other bind address refuses to start without
`--admin-token`, which every request, `/healthz` included, must then carry
as `Authorization: Bearer <token>` (compared in constant time; a missing or
wrong token is 401 `{"error":"missing or invalid bearer token"}`). The check
happens before anything is bound, like the frame-limit check, and the admin
address is bound before the protocol listener, so a taken port fails the
start with nothing left running. There is no TLS on the admin listener: on a shared network bind loopback and reach it
through an SSH tunnel, or terminate TLS in front of it.

The status view lists every party with its bind address and rendezvous key.
That is an operator's view of the whole mesh, which is why it sits behind
loopback or a token and not on the protocol listener, where any party could
ask.

| Method and path | Result |
|---|---|
| `GET /healthz` | `{"ok":true}` |
| `GET /metrics` | the Prometheus text exposition, `Content-Type: text/plain; version=0.0.4; charset=utf-8` |
| `GET /v1/status` | the status document below, as JSON |

Anything else is 404.

## Metrics

Every metric is prefixed `nsm_`; counters end in `_total`; units are in the
name (`_seconds`, `_bytes`). Gauges are read from the broker's registry at
the moment of the scrape, under the same lock every operation takes, so
they are always exact. Counters count since the broker started
(`nsm_start_time_seconds` says when) and reset with it; `rate()` and
`increase()` handle that.

Labels come from closed sets, listed below. No metric carries a rendezvous
key, a host or a party id as a label: those are chosen by users, a job
array can invent thousands, and every distinct label value is a time series
Prometheus keeps. Per-key and per-host counts are in the status document
instead.

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `nsm_build_info` | gauge (always 1) | `version`, `protocol_version` | the running binary |
| `nsm_start_time_seconds` | gauge | | when the broker started, in seconds since the Unix epoch |
| `nsm_parties` | gauge | `role` = `service` \| `client`, `mode` = `heartbeat` \| `ping` | registered parties |
| `nsm_services_unclaimed` | gauge | | services no client holds |
| `nsm_keys` | gauge | | distinct rendezvous keys with at least one party |
| `nsm_parties_failing` | gauge | | parties with at least one consecutive failed heartbeat |
| `nsm_heartbeat_tasks` | gauge | | two-sided parties the broker is dialling |
| `nsm_registrations_limit` | gauge | | `--max-registrations` |
| `nsm_stores` | gauge | | shared stores (one per client) |
| `nsm_store_entries` | gauge | | entries over every store |
| `nsm_store_bytes` | gauge | | accounted bytes over every store (see `--max-store-bytes` for the accounting) |
| `nsm_store_bytes_limit` | gauge | | `--max-store-bytes`, the budget of one store |
| `nsm_requests_total` | counter | `kind` = `publish` \| `claim` \| `ping` \| `deliver` \| `store_relay` \| `other`, `outcome` = `ok` \| `nack` \| `error` | requests the broker answered |
| `nsm_registrations_total` | counter | `role` | registrations granted |
| `nsm_registrations_refused_total` | counter | `reason` = `full` \| `per_host` \| `host_mismatch` \| `bad_port` \| `no_service` | registrations refused |
| `nsm_removals_total` | counter | `role`, `reason` = `heartbeats_failed` \| `no_ping` \| `no_replacement` | parties removed |
| `nsm_repairings_total` | counter | | clients re-paired with another service after theirs vanished |
| `nsm_heartbeats_total` | counter | `outcome` = `ack` \| `fail` | two-sided heartbeats the broker sent |
| `nsm_heartbeat_duration_seconds` | histogram | `le` | round trip of those heartbeats, failures and timeouts included; buckets at 5, 10, 25, 50, 100, 250 and 500 ms, 1, 2.5 and 5 s |
| `nsm_store_ops_total` | counter | `op` = `get` \| `put` \| `delete` \| `list`, `outcome` = `applied` \| `not_applied` \| `refused` | store operations relayed to the broker |

Some readings:

- **Is the mesh healthy?** `nsm_parties_failing` is 0 and
  `rate(nsm_heartbeats_total{outcome="fail"}[5m])` is near 0. A rising
  `histogram_quantile(0.99, rate(nsm_heartbeat_duration_seconds_bucket[5m]))`
  means the fabric between the broker and the compute nodes is slow before
  anything is removed.
- **Is the broker filling up?** `sum(nsm_parties) / nsm_registrations_limit`,
  and `nsm_registrations_refused_total{reason="full"}` once it has.
  `reason="per_host"` points at one node starting too many parties.
- **Are clients waiting for services?** `nsm_registrations_refused_total{reason="no_service"}`
  counts claims that found nothing under their key within the claim wait;
  `nsm_services_unclaimed` counts services nobody uses.
- **Who died?** `nsm_removals_total` by `role` and `reason`:
  `heartbeats_failed` is a two-sided party that stopped answering, `no_ping`
  a ping-mode party that fell silent, `no_replacement` a client whose
  service vanished with no spare under its key. `nsm_repairings_total`
  counts the clients that did find a spare.
- **How busy is the store?** `rate(nsm_store_ops_total[5m])` by `op`;
  `outcome="not_applied"` is the rate of lost compare-and-set races,
  `outcome="refused"` includes writes at a service nobody holds and puts
  that did not fit; `nsm_store_bytes / (nsm_stores * nsm_store_bytes_limit)`
  is how full the stores are on average.

## The status document

`GET /v1/status` answers one JSON object, the same the `nsm status` command
reads:

```json
{
  "version": "0.1.0",
  "protocol_version": 3,
  "started_at": 1790845077,
  "uptime_seconds": 3723,
  "bound": "10.0.0.1:12000",
  "limits": {"max_frame_bytes": 65536, "max_connections": 1024, "max_registrations": 10000,
             "max_store_bytes": 16384, "max_registrations_per_host": 64, "require_matching_host": false},
  "timing": {"heartbeat_interval": 2.0, "heartbeat_timeout": 3.0, "fail_threshold": 5,
             "ping_staleness": 20.0, "request_timeout": 6.0, "connect_timeout": 5.0, "claim_wait": 1.5},
  "counts": {"services": 3, "services_unclaimed": 1, "clients": 2, "ping_parties": 0,
             "heartbeat_tasks": 5, "keys": 2, "failing": 0, "store_entries": 4, "store_bytes": 420},
  "keys": [{"key": 1234, "services": 2, "unclaimed": 0, "clients": 2},
           {"key": 99, "services": 1, "unclaimed": 1, "clients": 0}],
  "hosts": [{"host": "10.0.0.7", "parties": 3}, {"host": "10.0.0.9", "parties": 2}],
  "totals": {
    "requests": {"publish": {"ok": 3, "nack": 0, "error": 0}, "claim": {"ok": 2, "nack": 1, "error": 0}, "...": {}},
    "registrations": {"service": 3, "client": 2},
    "registrations_refused": {"full": 0, "per_host": 0, "host_mismatch": 0, "bad_port": 0, "no_service": 1},
    "removals": {"service": {"heartbeats_failed": 0, "no_ping": 0, "no_replacement": 0}, "client": {"...": 0}},
    "repairings": 0,
    "heartbeats": {"ack": 9300, "fail": 2},
    "heartbeat_seconds": {"count": 9302, "sum": 18.4, "buckets": [{"le": "0.005", "count": 9100}, {"le": "+Inf", "count": 9302}]},
    "store_ops": {"get": {"applied": 40, "not_applied": 0, "refused": 0}, "...": {}}
  },
  "parties": [
    {"id": 1, "role": "service", "key": 1234, "bind_addr": "10.0.0.7:12010", "ping": false,
     "failures": 0, "paired_with": 4, "last_seen_seconds_ago": 1},
    {"id": 4, "role": "client", "key": 1234, "bind_addr": "10.0.0.9:12020", "ping": false,
     "failures": 0, "paired_with": 1, "last_seen_seconds_ago": 0}
  ]
}
```

`bound` is the protocol listener's address with its transport (`host:port`
for TCP, `tls://`, `http://` or `https://` otherwise). `counts` are the
gauges, `totals` the counters, keyed by the label values of the table above;
`keys` and `hosts` are the per-key and per-host breakdowns the metrics do
not carry; `parties` is every registration, services first, ascending by id,
with `paired_with` naming a service's client or a client's service and
`last_seen_seconds_ago` the time since the broker last heard from the party
(a registration, an acknowledged heartbeat or a ping). In the library the
document is `nsm::broker::Status`.

## Quick statistics from a shell

`nsm status ADMIN` reads the status document and prints it as one block;
`ADMIN` is the admin listener's address (`host:port` or `http://host:port`),
and `--admin-token` (`NSM_ADMIN_TOKEN`) is the broker's token when it has
one.

```bash
nsm status 127.0.0.1:9108
nsm status 127.0.0.1:9108 --parties          # plus one row per registered party
nsm status 127.0.0.1:9108 --json             # the document as one line, for scripts
nsm status 127.0.0.1:9108 --watch 5          # again every 5 seconds until Ctrl-C
```

```text
broker 127.0.0.1:12000   nsm 0.1.0, protocol 3   up 1h 02m 03s, since 2026-10-01T10:00:00Z
parties      3 services (1 unclaimed), 2 clients, 2 keys; 0 in ping mode, 5 heartbeat tasks, 0 failing; 5 of 10000 registrations
stores       2 stores, 4 entries, 420 bytes (16384 per store at most)
since start  registrations 5 granted, 1 refused (no_service 1)
             removals 0 services, 0 clients; re-pairings 0
             heartbeats 9300 acknowledged, 2 failed; mean round trip 2.0 ms
             requests 9315 answered, 3 refused, 0 failed
             store ops 40: 40 applied, 0 not applied, 0 refused
keys         key  services unclaimed clients
             1234        2         0       2
             99          1         1       0
hosts        host     parties
             10.0.0.7       3
             10.0.0.9       2
parties      id role    key  mode      bind           peer failures last seen
             1  service 1234 heartbeat 10.0.0.7:12010    4        0 1s ago
             4  client  1234 heartbeat 10.0.0.9:12020    1        0 0s ago
```

The header names the broker's protocol listener, its version and how long
it has been up. `parties` and `stores` are the gauges; the `since start`
lines are the counters, with the non-zero reasons in parentheses. The
`keys` and `hosts` tables are the breakdowns the metrics do not carry; a
key with unclaimed services and no clients is a service nobody uses, a host
near `--max-registrations-per-host` is a node about to be refused. In the
`parties` table (with `--parties`), `peer` is a service's client or a
client's service (`-` for an unclaimed service), `failures` the consecutive
failed heartbeats, and `last seen` the time since the broker last heard
from the party.

Stdout carries only the summary (or the JSON), so it can be captured. With
`--watch` each text block is preceded by `--- <UTC timestamp>` and the
blocks are separated by a blank line, nothing is cleared, so the output can
go to a file; `--json --watch` prints one document per line. The exit
status is 0, 1 when the admin listener cannot be reached, refuses the token
or answers something else (`nsm: ...` on stderr), 2 for a usage error, which
includes a `tls://` or `https://` address: the admin listener speaks plain
HTTP. The request and connect timeouts are the usual `--request-timeout`
and `--connect-timeout`.

## Scraping

A minimal `prometheus.yml` for a broker on the same host:

```yaml
global:
  scrape_interval: 15s
scrape_configs:
  - job_name: nsm
    static_configs:
      - targets: ["127.0.0.1:9108"]
```

With a token (a broker bound off loopback), keep the token in a file
Prometheus can read and reference it, so it is in neither the configuration
nor the process list:

```yaml
scrape_configs:
  - job_name: nsm
    authorization:
      type: Bearer
      credentials_file: /etc/prometheus/nsm-admin-token
    static_configs:
      - targets: ["broker.example:9108"]
```

A scrape takes the registry lock once for the gauges and renders about 60
lines plus 11 per histogram bucket; it is cheap at any interval Prometheus
would use. Metric values are exact at the instant of the scrape.

## Running Prometheus and Grafana locally

Two ways to get a graph of a broker you are testing, both bound to the
host's loopback and both provisioning the same dashboard,
[`deploy/monitoring/grafana/dashboards/nsm.json`](../deploy/monitoring/grafana/dashboards/nsm.json)
(`NSM broker`: the stat row, parties by role and mode, registrations
granted and refused, removals and re-pairings, heartbeats and their round
trip quantiles, requests, store operations and bytes, with a `Broker`
variable when several brokers are scraped and an annotation at every
restart).

### On a laptop, with containers

[`deploy/monitoring/compose.yaml`](../deploy/monitoring/compose.yaml) runs
Prometheus on `127.0.0.1:9090` and Grafana on `127.0.0.1:3000` (user
`admin`, password `admin` unless `GRAFANA_ADMIN_PASSWORD` is set) with
Docker or Podman:

```bash
cd deploy/monitoring
docker compose up                # or: podman compose up
```

Prometheus scrapes a broker running on the host at
`host.docker.internal:9108`. A container reaches the host's interfaces,
not its loopback, so the broker's admin listener must bind every interface,
which needs a token; the stack presents the token from the file
`NSM_ADMIN_TOKEN_FILE` names, by default the committed
`admin-token.example` (`change-me`):

```bash
nsm listen --bind-port 12000 -i 127. --ip-version 4 --admin-bind 0.0.0.0:9108 --admin-token change-me
```

For a real token, write it to a file and name the file:

```bash
openssl rand -hex 16 > admin-token
nsm listen ... --admin-bind 0.0.0.0:9108 --admin-token "$(cat admin-token)"
NSM_ADMIN_TOKEN_FILE=./admin-token docker compose up
```

For a self-contained demo with nothing on the host, the `broker` profile
builds the image and runs a broker inside the stack with the same token,
reachable from the host at `http://127.0.0.1:12000`:

```bash
docker compose --profile broker up --build
nsm publish http://127.0.0.1:12000 --bind-port 12010 --service-port 9000 --key 1 -i 127. --ip-version 4
```

Then open <http://localhost:3000/d/nsm-broker>. Prometheus's own view of
the target is at <http://localhost:9090/targets>. Data is kept in two named
volumes; `docker compose down -v` removes it.

### On an interactive HPC node, without containers

[`scripts/monitoring-local.sh`](../scripts/monitoring-local.sh) runs the
Prometheus and Grafana binaries directly, as the current user, with
everything (configuration, data, logs, pids) in one work directory, by
default `$SCRATCH/nsm-monitoring` (then `$TMPDIR/nsm-monitoring`). Nothing
touches `$HOME` and nothing needs root.

```bash
scripts/monitoring-local.sh fetch     # once: download both release tarballs into the work directory
scripts/monitoring-local.sh start     # write the configuration, start both on loopback, print the URLs
scripts/monitoring-local.sh status
scripts/monitoring-local.sh stop
```

`fetch` is the only step that needs the network (run it on a login node):
it downloads the official release tarballs for the host's platform
(Prometheus from GitHub, Grafana from grafana.com, Linux or macOS, x86-64
or arm64), checks each against its published SHA-256 sum, and extracts
them into the work directory. If `prometheus` and `grafana` are already on
`PATH` (a Homebrew install, a module), `fetch` is not needed; `PROMETHEUS_BIN`,
`GRAFANA_BIN` and `GRAFANA_HOME` name them explicitly otherwise.

`start` scrapes `NSM_ADMIN` (default `127.0.0.1:9108`), presenting the
token in `NSM_ADMIN_TOKEN_FILE` when set, and binds Prometheus to
`127.0.0.1:$PROMETHEUS_PORT` (9090) and Grafana to
`127.0.0.1:$GRAFANA_PORT` (3000). On a compute node the broker's admin
listener on loopback is enough, with no token. To look at the graphs from
a laptop, forward the two ports over SSH; `start` prints the command:

```bash
ssh -L 3000:localhost:3000 -L 9090:localhost:9090 nid001234   # through the login node if needed
```

and open <http://localhost:3000/d/nsm-broker>. Every variable the script
reads is listed at its top (`scripts/monitoring-local.sh help`).

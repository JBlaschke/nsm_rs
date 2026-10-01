# Monitoring: plan

> Written on 2026-10-01 against `main` at `d11cfbd7` (after the shared-store
> work, [#12](https://github.com/JBlaschke/nsm_rs/pull/12)). Executed on the
> stacked branches `metrics/01` to `metrics/04`. Section 0 tracks the state of
> each branch; section 2 lists the decisions with what was taken and what to
> change if you disagree; section 3 says what each branch contains; section 4
> what is left out on purpose. The plan before this one is the
> [shared store](history/2026-shared-store/PLAN.md).

## 0. Status

| Branch | State | Notes |
|---|---|---|
| `metrics/01-broker-metrics` | done | `broker::metrics`: counters bumped where things happen, gauges read from the registry, the Prometheus text exposition written by hand, and `Status`, the JSON view; nothing is served yet |
| `metrics/02-admin-listener` | in progress | `nsm listen --admin-bind ADDR [--admin-token T]`: a plain-HTTP admin listener with `GET /metrics`, `GET /v1/status` and `GET /healthz`; loopback or token (D9); `docs/MONITORING.md` |
| `metrics/03-status-cli` | planned | `nsm status ADMIN [--json] [--parties] [--watch SECS]`: quick usage statistics from `/v1/status` |
| `metrics/04-local-stack` | planned | `deploy/monitoring/`: Prometheus and Grafana by compose with a provisioned dashboard; `scripts/monitoring-local.sh` for a laptop or an interactive HPC node without containers |

Each branch builds on the previous one and passes the checks in
[`CONTRIBUTING.md`](../CONTRIBUTING.md) at its tip.

## 1. The request

> Please give me a way to monitor nsm activity. I am thinking: a
> grafana/prometheus exporter, with help running grafana/prometheus locally
> (e.g. when testing on a laptop or interactive HPC nodes); and a basic CLI
> for quick usage statistics.

Today the only window into a running broker is its log (`NSM_LOG_LEVEL=info`
prints registrations, removals and re-pairings as they happen) and
`Broker::snapshot()`, which exists for tests and has no front-end. There is
no way to ask a broker how many services and clients it holds, how many
registrations it has refused, whether heartbeats are failing, or how full the
stores are, and no way to see any of that over time.

Two front-ends over one source of numbers: a Prometheus endpoint for Grafana
(the time series), and a status command for a shell (the moment). Both read
the same broker-side state, so they can never disagree, and both are optional:
a broker started as today behaves as today.

## 2. Decisions

| # | Decision | Taken | If you disagree |
|---|---|---|---|
| M1 | **The numbers live at the broker, in one `Metrics` value owned by `Broker`.** Counters are atomics bumped at the point where the event happens (the handler for requests, registrations, refusals and store operations; the monitor for heartbeats, removals and re-pairings). Gauges are not stored at all: they are read from the registry when someone asks, under the same lock every operation takes. | As stated. A gauge computed from the registry cannot drift from the truth (no "decrement on removal" to forget), and the broker is the only process that knows the whole mesh: parties see only themselves. Counters as atomics cost nothing on the hot path and need no lock. | Keep the gauges as counters too (increment on publish, decrement on removal): cheaper per scrape, but every new removal path must remember them. Or derive everything from the log (a `tracing` layer feeding a metrics crate): fragile, and `warn` is the default level. |
| M2 | **The exposition is written by hand.** Prometheus text format 0.0.4: `# HELP`, `# TYPE`, counters, gauges and one histogram with fixed buckets, rendered by `Metrics::render`. No new dependency. | As stated. The format is three line shapes; a renderer with tests is about as long as the glue a metrics crate would need, and the vendored tree, `cargo deny` and the MSRV stay as they are (D1, D12). | `prometheus-client` (the OpenMetrics crate, pulls in `parking_lot`, `dtoa` and a derive macro) or `metrics` + `metrics-exporter-prometheus` (its own hyper server). Either needs a `cargo vendor` commit and a `deny.toml` review. |
| M3 | **Every metric is `nsm_*` with fixed, low-cardinality labels.** Counters end in `_total`, units are in the name (`_seconds`, `_bytes`), labels come from closed sets (`role`, `mode`, `kind`, `outcome`, `reason`, `op`, `state`). No label carries a rendezvous key, a host or a party id. | As stated. Keys and hosts are chosen by users; a job array can invent thousands, and every distinct label value is a time series Prometheus keeps forever. Per-key and per-host counts are in the status JSON instead, where they cost nothing after the request. | A `key` label behind a flag (`--metrics-per-key`) for sites with few, stable keys. Additive later. |
| M4 | **What is measured** is the table in section 2.1: how many parties of which role and mode, how many services are unclaimed, how many parties are failing heartbeats, stores and their bytes against the limit; and since the broker started, requests by kind and outcome, registrations and refusals by reason, removals by role and reason, re-pairings, heartbeats by outcome with a latency histogram, and store operations by kind and outcome. Plus `nsm_build_info` and `nsm_start_time_seconds`. | As stated. Everything in the table answers an operator's question ("is the broker full", "are compute nodes answering", "did the service die or the client", "how fast are heartbeats across the fabric"). | Add connection and byte counts from the transport (needs the metrics handle plumbed below `broker`, against the import direction) or request latencies (the handler is a lock and a map; the latency is the network's, which the heartbeat histogram shows). |
| M5 | **One JSON view, `broker::Status`,** serialised by serde and shared by the admin route and the CLI (same crate, same struct): version and protocol version, start time and uptime, the bound address, the limits and timing in force, the current counts (with a per-key and a per-host breakdown), the totals since start, and the party rows of `snapshot()` with how long ago each was last heard from. | As stated. The admin route and `nsm status` cannot disagree about a field name, and `--json` gives scripts the whole picture in one line. Party bind addresses are in it: this is an operator's view, reachable only on loopback or with the token (M6). | A second, smaller JSON for the CLI; or no JSON at all and let the CLI parse `/metrics` (loses the per-key breakdown and the party rows). |
| M6 | **A separate admin listener, off by default.** `nsm listen --admin-bind ADDR` starts a plain-HTTP (axum) listener with `GET /metrics` (text), `GET /v1/status` (JSON) and `GET /healthz`. D9 applies as to `nsm serve`: a non-loopback bind requires `--admin-token` (`NSM_ADMIN_TOKEN`), presented as `Authorization: Bearer`, checked on every route. The protocol listener is untouched. | As stated. Prometheus scrapes HTTP, and the broker's protocol listener may be raw TCP or TLS; a second socket is the only way that works for every transport. Off by default, because a fixed default port would collide when two brokers share a login node and the tests start dozens. Parties never see this listener, so a party cannot list the mesh. | Serve `/metrics` on the protocol listener when it is HTTP (then not for `tcp`/`tls`), or a `status` message in the protocol (then any party can list everyone, and Prometheus still cannot read it). TLS on the admin listener is a follow-up (section 4). |
| M7 | **`nsm status ADMIN`** prints a summary (one block of counts, then the per-key table); `--parties` adds one row per party; `--json` prints the body of `/v1/status` as one line; `--watch SECS` repeats, each block preceded by a blank line and a timestamp, without clearing the screen. `ADMIN` is `host:port` or `http://host:port`; `--admin-token` (`NSM_ADMIN_TOKEN`) as for the broker. Exit 0, 1 when the admin listener cannot be reached or refuses, 2 for a usage error. | As stated. Stdout carries only the result (D5); `--watch` output that is not cleared can be redirected to a file; a token in the environment keeps it out of `ps`. | `nsm status` talking to the protocol port (needs M6's alternative); a curses view; a `--metrics` flag that prints the exposition (use `curl`). |
| M8 | **Local Prometheus and Grafana by compose**, under `deploy/monitoring/`: a `compose.yaml` with Prometheus and Grafana bound to loopback, a provisioned datasource and one provisioned dashboard, a `prometheus.yml` scraping the host's broker at `host.docker.internal:9108` through `extra_hosts: host-gateway` (works for Docker and Podman), and a `broker` profile that builds the image and runs a broker inside the stack for a self-contained demo. The token, when used, comes from a file Prometheus reads (`credentials_file`). | As stated. One `docker compose up` (or `podman compose up`) and a browser on `localhost:3000`. The dashboard JSON is committed, so it is versioned with the metrics it reads. | Grafana Agent or Alloy pushing to a hosted Grafana; a `docker run` one-liner per tool with no provisioning. |
| M9 | **A script for hosts without containers**: `scripts/monitoring-local.sh start|stop|status|fetch` runs the Prometheus and Grafana binaries directly (found on `PATH` or under `PROMETHEUS_HOME`/`GRAFANA_HOME`), writes the same configuration and provisioning into a work directory (`$NSM_MONITORING_DIR`, default under `$SCRATCH` or `$TMPDIR`), binds both to loopback on chosen ports, records pids, and prints the URLs and the `ssh -L` command to reach them from a laptop. `fetch` downloads the official release tarballs for the host's platform into the work directory and checks their published SHA-256 sums; it is the only step that needs the network. | As stated. Interactive HPC nodes have no Docker, often no Podman, and no root; a login node usually has outbound HTTPS. Everything the script writes lives in the work directory and nothing touches `$HOME`. | Apptainer/Shifter/podman-hpc definitions (site-specific; the compose file is a start); a Spack or Conda recipe. |
| M10 | **Nothing changes on the wire, and nothing is persisted.** `PROTOCOL_VERSION` stays 3; parties are untouched; counters start at zero with the broker and `nsm_start_time_seconds` says when. `nsm serve` gets no metrics in this round. | As stated. Prometheus handles counter resets (`rate()` and `increase()` are reset-aware), and a broker restart already loses every registration. | Counters checkpointed to disk (then a restart's resets vanish from graphs, but stale numbers survive a crash); metrics on the control plane for its jobs (additive later, with the same module). |

### 2.1 Metrics

Gauges are read from the registry at scrape time; counters count since the
broker started. Buckets of the histogram: 5 ms, 10, 25, 50, 100, 250, 500 ms,
1 s, 2.5 s, 5 s and `+Inf`.

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `nsm_build_info` | gauge (1) | `version`, `protocol_version` | the running binary |
| `nsm_start_time_seconds` | gauge | | when the broker started, Unix seconds |
| `nsm_parties` | gauge | `role` (`service`/`client`), `mode` (`heartbeat`/`ping`) | registered parties |
| `nsm_services_unclaimed` | gauge | | services no client holds |
| `nsm_keys` | gauge | | distinct rendezvous keys with at least one party |
| `nsm_parties_failing` | gauge | | parties with at least one consecutive failed heartbeat |
| `nsm_heartbeat_tasks` | gauge | | two-sided parties the broker is dialling |
| `nsm_registrations_limit` | gauge | | `--max-registrations` |
| `nsm_stores` | gauge | | stores (one per client) |
| `nsm_store_entries` | gauge | | entries over every store |
| `nsm_store_bytes` | gauge | | accounted bytes over every store |
| `nsm_store_bytes_limit` | gauge | | `--max-store-bytes`, per store |
| `nsm_requests_total` | counter | `kind` (`publish`, `claim`, `ping`, `deliver`, `store_relay`, `other`), `outcome` (`ok`, `nack`, `error`) | requests the handler answered |
| `nsm_registrations_total` | counter | `role` | registrations granted |
| `nsm_registrations_refused_total` | counter | `reason` (`full`, `per_host`, `host_mismatch`, `bad_port`, `no_service`) | registrations refused |
| `nsm_removals_total` | counter | `role`, `reason` (`heartbeats_failed`, `no_ping`, `no_replacement`) | parties removed |
| `nsm_repairings_total` | counter | | clients re-paired after their service vanished |
| `nsm_heartbeats_total` | counter | `outcome` (`ack`, `fail`) | two-sided heartbeats the broker sent |
| `nsm_heartbeat_duration_seconds` | histogram | | round trip of those heartbeats, failures included |
| `nsm_store_ops_total` | counter | `op` (`get`, `put`, `delete`, `list`), `outcome` (`applied`, `not_applied`, `refused`) | store operations relayed to the broker |

## 3. Branches

### `metrics/01-broker-metrics`

- `broker::metrics`: `Metrics` (atomic counters indexed by small enums, one
  histogram), `Metrics::render(&Registry, ...)` writing the text exposition,
  `Status` and `Broker::status()`; `RemovalReason` replaces the free-text
  reason of `Broker::drop_party`.
- Hooks: `BrokerHandler` counts requests, registrations, refusals and store
  operations; the monitor counts heartbeats with their round trip, removals
  and re-pairings.
- Tests: the exposition format (exact text for a known state, escaping,
  every `# TYPE` once), the histogram's buckets, counters through the
  handler and the monitor's scripted peers, gauges against the registry.
- Docs: this plan; `docs/ARCHITECTURE.md` (components); `CHANGELOG.md`.

### `metrics/02-admin-listener`

- `broker::admin`: `AdminOpts { bind, token }`, the axum router (`/metrics`,
  `/v1/status`, `/healthz`), the bearer check shared in spirit with `rest`,
  `serve()` on a `TcpListener` under the broker's shutdown token.
- `ListenOpts.admin: Option<AdminOpts>`, `ListenRequest.admin`,
  `BrokerHandle::admin_addr()`; `--admin-bind` and `--admin-token`
  (`NSM_ADMIN_TOKEN`) on `nsm listen`; the loopback-or-token check before
  anything binds.
- Tests: `tests/e2e.rs` scrapes a cluster's admin listener and checks the
  gauges and counters against what the test did; the token; `tests/cli.rs`
  starts a broker with `--admin-bind` and reads `/healthz`.
- Docs: `docs/MONITORING.md` (the endpoints, the metric table, scrape
  configuration, what to alert on), `docs/ARCHITECTURE.md` (roles, layout,
  configuration, security model), README (command table, admin options,
  deployment, docs table), the Pages landing page, `CHANGELOG.md`.

### `metrics/03-status-cli`

- `ops::status(admin, token, net) -> Result<Status>` over reqwest.
- `nsm status ADMIN [--json] [--parties] [--watch SECS] [--admin-token T]`
  and its printing in `main`.
- Tests: parsing; `tests/cli.rs` runs a broker with an admin listener, a
  service and a client, and checks the summary, `--parties`, `--json`, the
  token and the exit codes.
- Docs: README (command table, quickstart), `docs/MONITORING.md`,
  `CHANGELOG.md`.

### `metrics/04-local-stack`

- `deploy/monitoring/compose.yaml`, `prometheus.yml`, the Grafana
  provisioning (datasource, dashboard provider) and `dashboards/nsm.json`.
- `scripts/monitoring-local.sh`.
- Docs: `docs/MONITORING.md` (laptop with containers, interactive node
  without, port forwarding), README (deployment), `CHANGELOG.md`.

## 4. Out of scope

- TLS on the admin listener and `https://` in `nsm status`. The admin
  listener follows `nsm serve`: loopback by default, a bearer token
  elsewhere, over plain HTTP. On a shared network, bind loopback and let
  Prometheus reach it through an SSH tunnel, or terminate TLS in front of
  it. Adding `--admin-tls` later reuses `TlsOpts`.
- Metrics at parties and at `nsm serve` (its jobs). The module is reusable;
  the control plane would mount `/metrics` on its own router.
- Per-key or per-host labels on metrics (M3); connection and byte counts
  from the transport (M4); request latencies at the handler.
- Alerting rules and recording rules. `docs/MONITORING.md` suggests what to
  alert on; the rules depend on the site's timing values.
- Pushing metrics (Pushgateway, remote write) and OpenTelemetry.
- Persisting counters across restarts (M10).
- Site-specific container definitions (Apptainer, Shifter, podman-hpc) for
  the monitoring stack.

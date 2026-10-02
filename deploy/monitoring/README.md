# Local monitoring stack

Prometheus and Grafana for watching an nsm broker on a laptop, bound to
loopback, with the datasource and the `NSM broker` dashboard provisioned.
How to use it, and how to run the same two servers without containers on
an interactive HPC node (`scripts/monitoring-local.sh`), is described in
[`docs/MONITORING.md`](../../docs/MONITORING.md#running-prometheus-and-grafana-locally).

| File | Purpose |
|---|---|
| `compose.yaml` | Prometheus and Grafana on `127.0.0.1:9090` and `127.0.0.1:3000`; an optional `broker` profile that runs a broker inside the stack |
| `prometheus.yml` | scrapes the host's broker at `host.docker.internal:9108` and the profile's broker at `broker:9108`, presenting the token from the mounted file |
| `admin-token.example` | the token the stack presents unless `NSM_ADMIN_TOKEN_FILE` names another file (`change-me`) |
| `grafana/provisioning/` | the datasource (uid `prometheus`) and the dashboard provider |
| `grafana/dashboards/nsm.json` | the dashboard: parties, registrations, removals, heartbeats and their round trip, requests, the stores |

```bash
docker compose up                        # or: podman compose up
docker compose --profile broker up --build
```

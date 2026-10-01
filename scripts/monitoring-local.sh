#!/usr/bin/env sh
# Run Prometheus and Grafana against an nsm broker without containers: on a
# laptop, or on an interactive HPC node where there is no Docker and no root.
# Everything lives in one work directory (configuration, data, logs, pids);
# both servers bind loopback. See docs/MONITORING.md.
#
#   scripts/monitoring-local.sh fetch      # download the two release tarballs into the work directory (needs the network)
#   scripts/monitoring-local.sh start      # write the configuration, start both, print the URLs
#   scripts/monitoring-local.sh status     # are they running, are they ready
#   scripts/monitoring-local.sh stop
#
# Environment (every value has a default):
#   NSM_ADMIN              the broker's admin listener to scrape      127.0.0.1:9108
#   NSM_ADMIN_TOKEN_FILE   file holding the broker's --admin-token   none (no token)
#   NSM_MONITORING_DIR     the work directory                        $SCRATCH/nsm-monitoring, else $TMPDIR/nsm-monitoring
#   NSM_MONITORING_SRC     the repository's deploy/monitoring        next to this script
#   PROMETHEUS_PORT        Prometheus port on 127.0.0.1              9090
#   GRAFANA_PORT           Grafana port on 127.0.0.1                 3000
#   PROMETHEUS_BIN         the prometheus binary                     $PATH, then the work directory (after fetch)
#   GRAFANA_BIN            the grafana binary                        $PATH, then the work directory (after fetch)
#   GRAFANA_HOME           Grafana's home (conf/, public/)           derived from the binary; Homebrew's share/grafana
#   PROMETHEUS_VERSION     version `fetch` downloads                 3.15.0
#   GRAFANA_VERSION        version `fetch` downloads                 stable (asked of grafana.com)
set -eu

here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
dir=${NSM_MONITORING_DIR:-${SCRATCH:-${TMPDIR:-/tmp}}/nsm-monitoring}
src=${NSM_MONITORING_SRC:-$here/../deploy/monitoring}
target=${NSM_ADMIN:-127.0.0.1:9108}
token_file=${NSM_ADMIN_TOKEN_FILE:-}
prom_port=${PROMETHEUS_PORT:-9090}
graf_port=${GRAFANA_PORT:-3000}
prom_version=${PROMETHEUS_VERSION:-3.15.0}
graf_version=${GRAFANA_VERSION:-stable}

say() { printf '%s\n' "$*"; }
die() { printf 'monitoring-local: %s\n' "$*" >&2; exit 1; }

usage() {
  sed -n '2,/^set -eu/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
}

# ----- locating the binaries ---------------------------------------------------

find_prometheus() {
  if [ -n "${PROMETHEUS_BIN:-}" ]; then say "$PROMETHEUS_BIN"; return; fi
  if command -v prometheus >/dev/null 2>&1; then command -v prometheus; return; fi
  for p in "$dir"/prometheus-*/prometheus; do
    [ -x "$p" ] && { say "$p"; return; }
  done
  die "no prometheus binary: put one on PATH, set PROMETHEUS_BIN, or run '$0 fetch'"
}

find_grafana() {
  if [ -n "${GRAFANA_BIN:-}" ]; then say "$GRAFANA_BIN"; return; fi
  for name in grafana grafana-server; do
    if command -v "$name" >/dev/null 2>&1; then command -v "$name"; return; fi
  done
  for g in "$dir"/grafana-*/bin/grafana "$dir"/grafana-*/bin/grafana-server; do
    [ -x "$g" ] && { say "$g"; return; }
  done
  die "no grafana binary: put one on PATH, set GRAFANA_BIN, or run '$0 fetch'"
}

# Grafana needs its home directory (conf/defaults.ini, public/): the tarball's
# top directory, or Homebrew's share/grafana.
find_grafana_home() {
  bin=$1
  if [ -n "${GRAFANA_HOME:-}" ]; then say "$GRAFANA_HOME"; return; fi
  resolved=$bin
  while [ -L "$resolved" ]; do
    link=$(readlink "$resolved")
    case $link in /*) resolved=$link ;; *) resolved=$(dirname -- "$resolved")/$link ;; esac
  done
  candidate=$(CDPATH='' cd -- "$(dirname -- "$resolved")/.." && pwd)
  if [ -f "$candidate/conf/defaults.ini" ]; then say "$candidate"; return; fi
  if command -v brew >/dev/null 2>&1; then
    brewed=$(brew --prefix 2>/dev/null)/share/grafana
    if [ -f "$brewed/conf/defaults.ini" ]; then say "$brewed"; return; fi
  fi
  die "cannot find Grafana's home (the directory with conf/defaults.ini) for $bin; set GRAFANA_HOME"
}

# ----- configuration -----------------------------------------------------------

write_config() {
  [ -f "$src/grafana/dashboards/nsm.json" ] ||
    die "no dashboard at $src/grafana/dashboards/nsm.json; set NSM_MONITORING_SRC to the repository's deploy/monitoring"
  mkdir -p "$dir/prometheus-data" "$dir/grafana/data" "$dir/grafana/logs" "$dir/grafana/plugins" \
    "$dir/grafana/provisioning/datasources" "$dir/grafana/provisioning/dashboards" "$dir/grafana/dashboards"
  {
    say "# Written by $0; edit the environment, not this file."
    say "global:"
    say "  scrape_interval: 5s"
    say "scrape_configs:"
    say "  - job_name: nsm"
    if [ -n "$token_file" ]; then
      [ -r "$token_file" ] || die "cannot read NSM_ADMIN_TOKEN_FILE=$token_file"
      say "    authorization:"
      say "      type: Bearer"
      say "      credentials_file: $token_file"
    fi
    say "    static_configs:"
    say "      - targets: [\"$target\"]"
  } > "$dir/prometheus.yml"
  {
    say "apiVersion: 1"
    say "datasources:"
    say "  - name: Prometheus"
    say "    uid: prometheus"
    say "    type: prometheus"
    say "    access: proxy"
    say "    url: http://127.0.0.1:$prom_port"
    say "    isDefault: true"
    say "    editable: false"
    say "    jsonData:"
    say "      timeInterval: 5s"
  } > "$dir/grafana/provisioning/datasources/prometheus.yml"
  {
    say "apiVersion: 1"
    say "providers:"
    say "  - name: nsm"
    say "    type: file"
    say "    updateIntervalSeconds: 30"
    say "    allowUiUpdates: true"
    say "    options:"
    say "      path: $dir/grafana/dashboards"
  } > "$dir/grafana/provisioning/dashboards/nsm.yml"
  cp "$src/grafana/dashboards/nsm.json" "$dir/grafana/dashboards/nsm.json"
}

# ----- processes ---------------------------------------------------------------

alive() {
  [ -f "$dir/$1.pid" ] && kill -0 "$(cat "$dir/$1.pid")" 2>/dev/null
}

start() {
  prom=$(find_prometheus)
  graf=$(find_grafana)
  graf_home=$(find_grafana_home "$graf")
  mkdir -p "$dir"
  write_config
  if alive prometheus; then
    say "prometheus already running (pid $(cat "$dir/prometheus.pid"))"
  else
    nohup "$prom" \
      --config.file="$dir/prometheus.yml" \
      --storage.tsdb.path="$dir/prometheus-data" \
      --storage.tsdb.retention.time=7d \
      --web.listen-address="127.0.0.1:$prom_port" \
      --web.enable-lifecycle \
      > "$dir/prometheus.log" 2>&1 &
    say $! > "$dir/prometheus.pid"
  fi
  if alive grafana; then
    say "grafana already running (pid $(cat "$dir/grafana.pid"))"
  else
    case $(basename -- "$graf") in
      grafana) set -- server ;;
      *) set -- ;;
    esac
    GF_PATHS_DATA="$dir/grafana/data" \
    GF_PATHS_LOGS="$dir/grafana/logs" \
    GF_PATHS_PLUGINS="$dir/grafana/plugins" \
    GF_PATHS_PROVISIONING="$dir/grafana/provisioning" \
    GF_SERVER_HTTP_ADDR=127.0.0.1 \
    GF_SERVER_HTTP_PORT="$graf_port" \
    GF_SECURITY_ADMIN_USER=admin \
    GF_SECURITY_ADMIN_PASSWORD="${GRAFANA_ADMIN_PASSWORD:-admin}" \
    GF_USERS_ALLOW_SIGN_UP=false \
    GF_ANALYTICS_REPORTING_ENABLED=false \
    GF_ANALYTICS_CHECK_FOR_UPDATES=false \
    GF_NEWS_NEWS_FEED_ENABLED=false \
    nohup "$graf" "$@" --homepath "$graf_home" > "$dir/grafana.log" 2>&1 &
    say $! > "$dir/grafana.pid"
  fi
  sleep 1
  alive prometheus || die "prometheus exited; see $dir/prometheus.log"
  alive grafana || die "grafana exited; see $dir/grafana.log"
  host=$(hostname)
  say "work directory  $dir"
  say "scraping        http://$target/metrics${token_file:+ (token from $token_file)}"
  say "prometheus      http://127.0.0.1:$prom_port   (pid $(cat "$dir/prometheus.pid"), log $dir/prometheus.log)"
  say "grafana         http://127.0.0.1:$graf_port   (pid $(cat "$dir/grafana.pid"), log $dir/grafana.log; admin / ${GRAFANA_ADMIN_PASSWORD:-admin})"
  say "from a laptop   ssh -L $graf_port:localhost:$graf_port -L $prom_port:localhost:$prom_port $host"
  say "                (through the login node if $host is a compute node), then open http://localhost:$graf_port"
  say "stop with       $0 stop"
}

stop() {
  for name in grafana prometheus; do
    if alive "$name"; then
      kill "$(cat "$dir/$name.pid")" && say "$name stopped"
    else
      say "$name not running"
    fi
    rm -f "$dir/$name.pid"
  done
}

ready() {
  command -v curl >/dev/null 2>&1 || return 0
  if curl -fsS --max-time 2 "$1" >/dev/null 2>&1; then say "ready"; else say "not ready"; fi
}

status() {
  for name in prometheus grafana; do
    if alive "$name"; then
      port=$prom_port; path=/-/ready
      [ "$name" = grafana ] && { port=$graf_port; path=/api/health; }
      say "$name running (pid $(cat "$dir/$name.pid")) on http://127.0.0.1:$port: $(ready "http://127.0.0.1:$port$path")"
    else
      say "$name not running"
    fi
  done
}

# ----- downloads -----------------------------------------------------------------

platform() {
  case $(uname -s) in
    Linux) os=linux ;;
    Darwin) os=darwin ;;
    *) die "unsupported OS $(uname -s)" ;;
  esac
  case $(uname -m) in
    x86_64|amd64) arch=amd64 ;;
    aarch64|arm64) arch=arm64 ;;
    *) die "unsupported architecture $(uname -m)" ;;
  esac
  say "$os-$arch"
}

download() {
  url=$1; out=$2
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL --retry 3 -o "$out" "$url"
  elif command -v wget >/dev/null 2>&1; then
    wget -q -O "$out" "$url"
  else
    die "need curl or wget to download $url"
  fi
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d' ' -f1
  else
    die "need sha256sum or shasum to verify downloads"
  fi
}

verify() {
  file=$1; expected=$2
  actual=$(sha256_of "$file")
  [ "$actual" = "$expected" ] || die "checksum mismatch for $file: expected $expected, got $actual"
  say "verified $(basename -- "$file")"
}

fetch() {
  plat=$(platform)
  os=${plat%-*}; arch=${plat#*-}
  mkdir -p "$dir/downloads"

  # Prometheus: a GitHub release with sha256sums.txt next to the tarballs.
  tarball="prometheus-$prom_version.$os-$arch.tar.gz"
  base="https://github.com/prometheus/prometheus/releases/download/v$prom_version"
  say "downloading $tarball"
  download "$base/$tarball" "$dir/downloads/$tarball"
  download "$base/sha256sums.txt" "$dir/downloads/prometheus-sha256sums.txt"
  expected=$(grep " $tarball\$" "$dir/downloads/prometheus-sha256sums.txt" | cut -d' ' -f1)
  [ -n "$expected" ] || die "sha256sums.txt lists no $tarball"
  verify "$dir/downloads/$tarball" "$expected"
  tar -xzf "$dir/downloads/$tarball" -C "$dir"

  # Grafana: grafana.com's versions API names the tarball for each platform
  # (its URL carries a build number) and its sha256.
  command -v python3 >/dev/null 2>&1 || die "need python3 to read grafana.com's versions API"
  download "https://grafana.com/api/grafana/versions/$graf_version" "$dir/downloads/grafana-version.json"
  pkg=$(python3 - "$dir/downloads/grafana-version.json" "$os" "$arch" <<'PY'
import json, sys
doc = json.load(open(sys.argv[1]))
suffix = f"_{sys.argv[2]}_{sys.argv[3]}.tar.gz"
for p in doc.get("packages", []):
    if p.get("url", "").endswith(suffix):
        print(p["url"], p["sha256"])
        break
PY
)
  [ -n "$pkg" ] || die "grafana.com lists no $os/$arch tarball for Grafana $graf_version"
  url=${pkg% *}; expected=${pkg#* }
  tarball=$(basename -- "$url")
  say "downloading $tarball"
  download "$url" "$dir/downloads/$tarball"
  verify "$dir/downloads/$tarball" "$expected"
  tar -xzf "$dir/downloads/$tarball" -C "$dir"

  say "extracted into $dir:"
  for extracted in "$dir"/prometheus-*/ "$dir"/grafana-*/; do
    [ -d "$extracted" ] && say "  $extracted"
  done
  say "next: $0 start"
}

case ${1:-} in
  start) start ;;
  stop) stop ;;
  status) status ;;
  fetch) fetch ;;
  -h|--help|help) usage ;;
  *) usage >&2; exit 2 ;;
esac

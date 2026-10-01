//! What a broker counts and how it reports it.
//!
//! [`Metrics`] is owned by the [`Broker`](super::Broker). Counters are
//! atomics bumped where the event happens (the handler for requests,
//! registrations, refusals and store operations; the monitor for heartbeats,
//! removals and re-pairings), so the hot path takes no lock for them.
//! Gauges are never stored: [`Gauges::of`] reads them from the registry when
//! someone asks, under the lock every operation takes, so a gauge cannot
//! drift from the truth (monitoring plan, decision M1).
//!
//! Two views exist over the same numbers: [`Metrics::render`] writes the
//! Prometheus text exposition (format 0.0.4, by hand, decision M2) and
//! [`Status`] is the JSON the admin route and `nsm status` share (decision
//! M5). Every metric is `nsm_*`, counters end in `_total`, units are in the
//! name, and every label comes from a closed set; no label carries a
//! rendezvous key, a host or a party id (decision M3). The per-key and
//! per-host breakdowns are in [`Status`] only.

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::registry::{Party, Registry};
use crate::config::{BrokerPolicy, Limits, Timing};
use crate::net::Addr;
use crate::protocol::{Key, Message, PROTOCOL_VERSION, PartyId, Role, StoreOp, Stored};

// ----- label sets -----------------------------------------------------------

/// The request kinds the broker counts, one per message it answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestKind {
    /// [`Message::Publish`].
    Publish,
    /// [`Message::Claim`].
    Claim,
    /// [`Message::Ping`].
    Ping,
    /// [`Message::Deliver`].
    Deliver,
    /// [`Message::StoreRelay`].
    StoreRelay,
    /// Anything else, which the broker refuses as unexpected.
    Other,
}

impl From<&Message> for RequestKind {
    fn from(msg: &Message) -> Self {
        match msg {
            Message::Publish { .. } => RequestKind::Publish,
            Message::Claim { .. } => RequestKind::Claim,
            Message::Ping { .. } => RequestKind::Ping,
            Message::Deliver { .. } => RequestKind::Deliver,
            Message::StoreRelay { .. } => RequestKind::StoreRelay,
            _ => RequestKind::Other,
        }
    }
}

impl RequestKind {
    const ALL: [RequestKind; 6] = [
        RequestKind::Publish,
        RequestKind::Claim,
        RequestKind::Ping,
        RequestKind::Deliver,
        RequestKind::StoreRelay,
        RequestKind::Other,
    ];

    /// The `kind` label.
    pub fn label(self) -> &'static str {
        match self {
            RequestKind::Publish => "publish",
            RequestKind::Claim => "claim",
            RequestKind::Ping => "ping",
            RequestKind::Deliver => "deliver",
            RequestKind::StoreRelay => "store_relay",
            RequestKind::Other => "other",
        }
    }
}

/// How the broker answered a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// A positive reply.
    Ok,
    /// A [`Message::Nack`].
    Nack,
    /// The handler failed; the transport reports that to the peer.
    Error,
}

impl Outcome {
    const ALL: [Outcome; 3] = [Outcome::Ok, Outcome::Nack, Outcome::Error];

    /// The `outcome` label.
    pub fn label(self) -> &'static str {
        match self {
            Outcome::Ok => "ok",
            Outcome::Nack => "nack",
            Outcome::Error => "error",
        }
    }
}

/// Why a registration was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalReason {
    /// The registry holds `max_registrations` parties.
    Full,
    /// The advertised host holds `max_registrations_per_host` parties.
    PerHost,
    /// The advertised host is not the connection's source and the policy
    /// requires it to be.
    HostMismatch,
    /// The advertised address carries port 0.
    BadPort,
    /// A claim found no unclaimed service under its key.
    NoService,
}

impl RefusalReason {
    const ALL: [RefusalReason; 5] = [
        RefusalReason::Full,
        RefusalReason::PerHost,
        RefusalReason::HostMismatch,
        RefusalReason::BadPort,
        RefusalReason::NoService,
    ];

    /// The `reason` label.
    pub fn label(self) -> &'static str {
        match self {
            RefusalReason::Full => "full",
            RefusalReason::PerHost => "per_host",
            RefusalReason::HostMismatch => "host_mismatch",
            RefusalReason::BadPort => "bad_port",
            RefusalReason::NoService => "no_service",
        }
    }
}

/// Why the broker removed a party. The argument of
/// [`Broker::drop_party`](super::Broker::drop_party); `Display` is the text
/// the log carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalReason {
    /// A two-sided party failed `fail_threshold` heartbeats in a row.
    HeartbeatsFailed,
    /// A ping-mode party was silent for longer than `ping_staleness`.
    NoPing,
    /// A client lost its service and no other service of its key was free.
    NoReplacement,
}

impl RemovalReason {
    const ALL: [RemovalReason; 3] = [
        RemovalReason::HeartbeatsFailed,
        RemovalReason::NoPing,
        RemovalReason::NoReplacement,
    ];

    /// The `reason` label.
    pub fn label(self) -> &'static str {
        match self {
            RemovalReason::HeartbeatsFailed => "heartbeats_failed",
            RemovalReason::NoPing => "no_ping",
            RemovalReason::NoReplacement => "no_replacement",
        }
    }
}

impl fmt::Display for RemovalReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            RemovalReason::HeartbeatsFailed => "heartbeats failed",
            RemovalReason::NoPing => "no ping received",
            RemovalReason::NoReplacement => "no replacement service",
        })
    }
}

/// How a two-sided heartbeat ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeartbeatOutcome {
    /// Acknowledged.
    Ack,
    /// Refused, answered with something else, failed or timed out.
    Fail,
}

impl HeartbeatOutcome {
    const ALL: [HeartbeatOutcome; 2] = [HeartbeatOutcome::Ack, HeartbeatOutcome::Fail];

    /// The `outcome` label.
    pub fn label(self) -> &'static str {
        match self {
            HeartbeatOutcome::Ack => "ack",
            HeartbeatOutcome::Fail => "fail",
        }
    }
}

/// The four store operations, as labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreOpKind {
    /// [`StoreOp::Get`].
    Get,
    /// [`StoreOp::Put`].
    Put,
    /// [`StoreOp::Delete`].
    Delete,
    /// [`StoreOp::List`].
    List,
}

impl StoreOpKind {
    const ALL: [StoreOpKind; 4] = [
        StoreOpKind::Get,
        StoreOpKind::Put,
        StoreOpKind::Delete,
        StoreOpKind::List,
    ];

    /// The `op` label, the same spelling as [`StoreOp::kind`].
    pub fn label(self) -> &'static str {
        match self {
            StoreOpKind::Get => "get",
            StoreOpKind::Put => "put",
            StoreOpKind::Delete => "delete",
            StoreOpKind::List => "list",
        }
    }
}

impl From<&StoreOp> for StoreOpKind {
    fn from(op: &StoreOp) -> Self {
        match op {
            StoreOp::Get { .. } => StoreOpKind::Get,
            StoreOp::Put { .. } => StoreOpKind::Put,
            StoreOp::Delete { .. } => StoreOpKind::Delete,
            StoreOp::List => StoreOpKind::List,
        }
    }
}

/// How a store operation ended at the broker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreOutcome {
    /// Answered and, for a write, applied.
    Applied,
    /// Answered with `applied: false`: the condition did not hold.
    NotApplied,
    /// Refused: wrong token, unclaimed service writing, store full.
    Refused,
}

impl StoreOutcome {
    const ALL: [StoreOutcome; 3] = [
        StoreOutcome::Applied,
        StoreOutcome::NotApplied,
        StoreOutcome::Refused,
    ];

    /// The `outcome` label.
    pub fn label(self) -> &'static str {
        match self {
            StoreOutcome::Applied => "applied",
            StoreOutcome::NotApplied => "not_applied",
            StoreOutcome::Refused => "refused",
        }
    }

    /// The outcome of an answered operation.
    pub fn of(stored: &Stored) -> StoreOutcome {
        if stored.applied {
            StoreOutcome::Applied
        } else {
            StoreOutcome::NotApplied
        }
    }
}

/// A party's liveness mode, as a label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The broker dials the party.
    Heartbeat,
    /// The party pings the broker.
    Ping,
}

impl Mode {
    const ALL: [Mode; 2] = [Mode::Heartbeat, Mode::Ping];

    /// The `mode` label.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Heartbeat => "heartbeat",
            Mode::Ping => "ping",
        }
    }

    fn of(ping: bool) -> Mode {
        if ping { Mode::Ping } else { Mode::Heartbeat }
    }
}

const ROLES: [Role; 2] = [Role::Service, Role::Client];

fn role_index(role: Role) -> usize {
    match role {
        Role::Service => 0,
        Role::Client => 1,
    }
}

// ----- the histogram ---------------------------------------------------------

/// Upper bounds of the heartbeat histogram's buckets, in seconds, with the
/// text the `le` label carries. `+Inf` is implicit.
const BUCKETS: [(f64, &str); 10] = [
    (0.005, "0.005"),
    (0.01, "0.01"),
    (0.025, "0.025"),
    (0.05, "0.05"),
    (0.1, "0.1"),
    (0.25, "0.25"),
    (0.5, "0.5"),
    (1.0, "1"),
    (2.5, "2.5"),
    (5.0, "5"),
];

/// A histogram of durations with fixed buckets (5 ms to 5 s, then
/// `+Inf`). Counts per bucket
/// (not cumulative) and the sum in microseconds, so that everything is an
/// integer until it is rendered.
#[derive(Debug, Default)]
pub struct Histogram {
    buckets: [AtomicU64; BUCKETS.len() + 1],
    sum_micros: AtomicU64,
}

impl Histogram {
    /// Record one duration.
    pub fn observe(&self, d: Duration) {
        let secs = d.as_secs_f64();
        let index = BUCKETS
            .iter()
            .position(|(bound, _)| secs <= *bound)
            .unwrap_or(BUCKETS.len());
        self.buckets[index].fetch_add(1, Ordering::Relaxed);
        let micros = u64::try_from(d.as_micros()).unwrap_or(u64::MAX);
        self.sum_micros.fetch_add(micros, Ordering::Relaxed);
    }

    /// Observations so far.
    pub fn count(&self) -> u64 {
        self.buckets
            .iter()
            .map(|b| b.load(Ordering::Relaxed))
            .fold(0, u64::saturating_add)
    }

    /// Sum of every observation, in seconds.
    pub fn sum_seconds(&self) -> f64 {
        self.sum_micros.load(Ordering::Relaxed) as f64 / 1e6
    }

    /// Cumulative counts, one per bucket bound plus `+Inf`, as the
    /// exposition and [`HistogramView`] report them.
    pub fn cumulative(&self) -> Vec<(&'static str, u64)> {
        let mut total = 0u64;
        self.buckets
            .iter()
            .enumerate()
            .map(|(i, b)| {
                total = total.saturating_add(b.load(Ordering::Relaxed));
                let le = BUCKETS.get(i).map_or("+Inf", |(_, le)| le);
                (le, total)
            })
            .collect()
    }
}

// ----- counters ---------------------------------------------------------------

/// Everything the broker counts since it started. Cheap to update from any
/// task; read by [`Metrics::render`] and [`Metrics::totals`].
#[derive(Debug)]
pub struct Metrics {
    started_wall: SystemTime,
    started: Instant,
    requests: [[AtomicU64; 3]; 6],
    registrations: [AtomicU64; 2],
    refused: [AtomicU64; 5],
    removals: [[AtomicU64; 3]; 2],
    repairings: AtomicU64,
    heartbeats: [AtomicU64; 2],
    heartbeat_seconds: Histogram,
    store_ops: [[AtomicU64; 3]; 4],
}

impl Default for Metrics {
    fn default() -> Self {
        Metrics::new()
    }
}

fn bump(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::Relaxed);
}

fn load(counter: &AtomicU64) -> u64 {
    counter.load(Ordering::Relaxed)
}

impl Metrics {
    /// Zeroed counters, started now.
    pub fn new() -> Self {
        Metrics {
            started_wall: SystemTime::now(),
            started: Instant::now(),
            requests: Default::default(),
            registrations: Default::default(),
            refused: Default::default(),
            removals: Default::default(),
            repairings: AtomicU64::new(0),
            heartbeats: Default::default(),
            heartbeat_seconds: Histogram::default(),
            store_ops: Default::default(),
        }
    }

    /// When the counters started, in seconds since the Unix epoch.
    pub fn started_at(&self) -> u64 {
        self.started_wall
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
    }

    /// How long ago the counters started.
    pub fn uptime(&self) -> Duration {
        self.started.elapsed()
    }

    /// One request answered.
    pub fn request(&self, kind: RequestKind, outcome: Outcome) {
        bump(&self.requests[kind as usize][outcome as usize]);
    }

    /// One registration granted.
    pub fn registered(&self, role: Role) {
        bump(&self.registrations[role_index(role)]);
    }

    /// One registration refused.
    pub fn refused(&self, reason: RefusalReason) {
        bump(&self.refused[reason as usize]);
    }

    /// One party removed.
    pub fn removed(&self, role: Role, reason: RemovalReason) {
        bump(&self.removals[role_index(role)][reason as usize]);
    }

    /// One client re-paired after its service vanished.
    pub fn repaired(&self) {
        bump(&self.repairings);
    }

    /// One two-sided heartbeat finished, after `took`.
    pub fn heartbeat(&self, outcome: HeartbeatOutcome, took: Duration) {
        bump(&self.heartbeats[outcome as usize]);
        self.heartbeat_seconds.observe(took);
    }

    /// One store operation answered or refused.
    pub fn store_op(&self, op: StoreOpKind, outcome: StoreOutcome) {
        bump(&self.store_ops[op as usize][outcome as usize]);
    }

    /// The heartbeat round-trip histogram.
    pub fn heartbeat_seconds(&self) -> &Histogram {
        &self.heartbeat_seconds
    }

    /// Requests answered with this kind and outcome.
    pub fn requests(&self, kind: RequestKind, outcome: Outcome) -> u64 {
        load(&self.requests[kind as usize][outcome as usize])
    }

    /// Registrations granted to this role.
    pub fn registrations(&self, role: Role) -> u64 {
        load(&self.registrations[role_index(role)])
    }

    /// Registrations refused for this reason.
    pub fn refusals(&self, reason: RefusalReason) -> u64 {
        load(&self.refused[reason as usize])
    }

    /// Parties of this role removed for this reason.
    pub fn removals(&self, role: Role, reason: RemovalReason) -> u64 {
        load(&self.removals[role_index(role)][reason as usize])
    }

    /// Clients re-paired so far.
    pub fn repairings(&self) -> u64 {
        load(&self.repairings)
    }

    /// Heartbeats that ended this way.
    pub fn heartbeats(&self, outcome: HeartbeatOutcome) -> u64 {
        load(&self.heartbeats[outcome as usize])
    }

    /// Store operations of this kind with this outcome.
    pub fn store_ops(&self, op: StoreOpKind, outcome: StoreOutcome) -> u64 {
        load(&self.store_ops[op as usize][outcome as usize])
    }

    /// The counters as the JSON view carries them.
    pub fn totals(&self) -> Totals {
        let mut requests: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
        for kind in RequestKind::ALL {
            let by_outcome = requests.entry(kind.label().to_owned()).or_default();
            for outcome in Outcome::ALL {
                by_outcome.insert(outcome.label().to_owned(), self.requests(kind, outcome));
            }
        }
        let mut removals: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
        for role in ROLES {
            let by_reason = removals.entry(role.to_string()).or_default();
            for reason in RemovalReason::ALL {
                by_reason.insert(reason.label().to_owned(), self.removals(role, reason));
            }
        }
        let mut store_ops: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
        for op in StoreOpKind::ALL {
            let by_outcome = store_ops.entry(op.label().to_owned()).or_default();
            for outcome in StoreOutcome::ALL {
                by_outcome.insert(outcome.label().to_owned(), self.store_ops(op, outcome));
            }
        }
        Totals {
            requests,
            registrations: ROLES
                .iter()
                .map(|r| (r.to_string(), self.registrations(*r)))
                .collect(),
            registrations_refused: RefusalReason::ALL
                .iter()
                .map(|r| (r.label().to_owned(), self.refusals(*r)))
                .collect(),
            removals,
            repairings: self.repairings(),
            heartbeats: HeartbeatOutcome::ALL
                .iter()
                .map(|o| (o.label().to_owned(), self.heartbeats(*o)))
                .collect(),
            heartbeat_seconds: HistogramView {
                count: self.heartbeat_seconds.count(),
                sum: self.heartbeat_seconds.sum_seconds(),
                buckets: self
                    .heartbeat_seconds
                    .cumulative()
                    .into_iter()
                    .map(|(le, count)| Bucket {
                        le: le.to_owned(),
                        count,
                    })
                    .collect(),
            },
            store_ops,
        }
    }

    /// The Prometheus text exposition (format 0.0.4) of these counters and
    /// the given gauges. Serve it as
    /// `text/plain; version=0.0.4; charset=utf-8`.
    pub fn render(&self, gauges: &Gauges) -> String {
        let mut e = Exposition::default();

        e.family("nsm_build_info", "The running binary (always 1).", "gauge");
        e.sample(
            "nsm_build_info",
            &[
                ("version", env!("CARGO_PKG_VERSION")),
                ("protocol_version", &PROTOCOL_VERSION.to_string()),
            ],
            1,
        );
        e.family(
            "nsm_start_time_seconds",
            "When the broker started, in seconds since the Unix epoch.",
            "gauge",
        );
        e.sample("nsm_start_time_seconds", &[], self.started_at());

        e.family(
            "nsm_parties",
            "Registered parties by role and mode.",
            "gauge",
        );
        for role in ROLES {
            for mode in Mode::ALL {
                e.sample(
                    "nsm_parties",
                    &[("role", &role.to_string()), ("mode", mode.label())],
                    gauges.parties(role, mode),
                );
            }
        }
        e.family(
            "nsm_services_unclaimed",
            "Services no client holds.",
            "gauge",
        );
        e.sample("nsm_services_unclaimed", &[], gauges.services_unclaimed);
        e.family(
            "nsm_keys",
            "Distinct rendezvous keys with at least one party.",
            "gauge",
        );
        e.sample("nsm_keys", &[], gauges.keys);
        e.family(
            "nsm_parties_failing",
            "Parties with at least one consecutive failed heartbeat.",
            "gauge",
        );
        e.sample("nsm_parties_failing", &[], gauges.failing);
        e.family(
            "nsm_heartbeat_tasks",
            "Two-sided parties the broker is dialling.",
            "gauge",
        );
        e.sample("nsm_heartbeat_tasks", &[], gauges.heartbeat_tasks);
        e.family(
            "nsm_registrations_limit",
            "Registrations the broker admits at once (--max-registrations).",
            "gauge",
        );
        e.sample("nsm_registrations_limit", &[], gauges.registrations_limit);
        e.family("nsm_stores", "Shared stores (one per client).", "gauge");
        e.sample("nsm_stores", &[], gauges.stores);
        e.family("nsm_store_entries", "Entries over every store.", "gauge");
        e.sample("nsm_store_entries", &[], gauges.store_entries);
        e.family(
            "nsm_store_bytes",
            "Accounted bytes over every store.",
            "gauge",
        );
        e.sample("nsm_store_bytes", &[], gauges.store_bytes);
        e.family(
            "nsm_store_bytes_limit",
            "Budget of one store in accounted bytes (--max-store-bytes).",
            "gauge",
        );
        e.sample("nsm_store_bytes_limit", &[], gauges.store_bytes_limit);

        e.family(
            "nsm_requests_total",
            "Requests the broker answered, by kind and outcome.",
            "counter",
        );
        for kind in RequestKind::ALL {
            for outcome in Outcome::ALL {
                e.sample(
                    "nsm_requests_total",
                    &[("kind", kind.label()), ("outcome", outcome.label())],
                    self.requests(kind, outcome),
                );
            }
        }
        e.family(
            "nsm_registrations_total",
            "Registrations granted, by role.",
            "counter",
        );
        for role in ROLES {
            e.sample(
                "nsm_registrations_total",
                &[("role", &role.to_string())],
                self.registrations(role),
            );
        }
        e.family(
            "nsm_registrations_refused_total",
            "Registrations refused, by reason.",
            "counter",
        );
        for reason in RefusalReason::ALL {
            e.sample(
                "nsm_registrations_refused_total",
                &[("reason", reason.label())],
                self.refusals(reason),
            );
        }
        e.family(
            "nsm_removals_total",
            "Parties removed, by role and reason.",
            "counter",
        );
        for role in ROLES {
            for reason in RemovalReason::ALL {
                e.sample(
                    "nsm_removals_total",
                    &[("role", &role.to_string()), ("reason", reason.label())],
                    self.removals(role, reason),
                );
            }
        }
        e.family(
            "nsm_repairings_total",
            "Clients re-paired after their service vanished.",
            "counter",
        );
        e.sample("nsm_repairings_total", &[], self.repairings());
        e.family(
            "nsm_heartbeats_total",
            "Two-sided heartbeats the broker sent, by outcome.",
            "counter",
        );
        for outcome in HeartbeatOutcome::ALL {
            e.sample(
                "nsm_heartbeats_total",
                &[("outcome", outcome.label())],
                self.heartbeats(outcome),
            );
        }
        e.family(
            "nsm_heartbeat_duration_seconds",
            "Round trip of two-sided heartbeats, failures included.",
            "histogram",
        );
        for (le, count) in self.heartbeat_seconds.cumulative() {
            e.sample(
                "nsm_heartbeat_duration_seconds_bucket",
                &[("le", le)],
                count,
            );
        }
        e.sample_f64(
            "nsm_heartbeat_duration_seconds_sum",
            self.heartbeat_seconds.sum_seconds(),
        );
        e.sample(
            "nsm_heartbeat_duration_seconds_count",
            &[],
            self.heartbeat_seconds.count(),
        );
        e.family(
            "nsm_store_ops_total",
            "Store operations relayed to the broker, by operation and outcome.",
            "counter",
        );
        for op in StoreOpKind::ALL {
            for outcome in StoreOutcome::ALL {
                e.sample(
                    "nsm_store_ops_total",
                    &[("op", op.label()), ("outcome", outcome.label())],
                    self.store_ops(op, outcome),
                );
            }
        }
        e.out
    }
}

/// Writer of the text exposition.
#[derive(Default)]
struct Exposition {
    out: String,
}

impl Exposition {
    fn family(&mut self, name: &str, help: &str, kind: &str) {
        // HELP text escapes a backslash and a newline only.
        let help = help.replace('\\', "\\\\").replace('\n', "\\n");
        let _ = writeln!(self.out, "# HELP {name} {help}");
        let _ = writeln!(self.out, "# TYPE {name} {kind}");
    }

    fn sample(&mut self, name: &str, labels: &[(&str, &str)], value: u64) {
        self.out.push_str(name);
        self.labels(labels);
        let _ = writeln!(self.out, " {value}");
    }

    fn sample_f64(&mut self, name: &str, value: f64) {
        let _ = writeln!(self.out, "{name} {value:.6}");
    }

    fn labels(&mut self, labels: &[(&str, &str)]) {
        if labels.is_empty() {
            return;
        }
        self.out.push('{');
        for (i, (name, value)) in labels.iter().enumerate() {
            if i > 0 {
                self.out.push(',');
            }
            let _ = write!(self.out, "{name}=\"{}\"", escape_label(value));
        }
        self.out.push('}');
    }
}

/// Escape a label value: backslash, double quote and newline.
fn escape_label(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out
}

// ----- gauges ----------------------------------------------------------------

/// One rendezvous key's parties, for [`Status`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyRow {
    /// The rendezvous key.
    pub key: Key,
    /// Services published under it.
    pub services: u64,
    /// Of those, services no client holds.
    pub unclaimed: u64,
    /// Clients that claimed it.
    pub clients: u64,
}

/// One advertised host's parties, for [`Status`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostRow {
    /// The host part of the parties' bind addresses.
    pub host: String,
    /// Parties advertising it, counted against the per-host cap.
    pub parties: u64,
}

/// The current state of the registry, as numbers. Read under the registry
/// lock by [`Gauges::of`]; `heartbeat_tasks` is filled in by the broker,
/// which owns the tasks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Gauges {
    parties: [[u64; 2]; 2],
    /// Services no client holds.
    pub services_unclaimed: u64,
    /// Distinct rendezvous keys with at least one party.
    pub keys: u64,
    /// Parties with at least one consecutive failed heartbeat.
    pub failing: u64,
    /// Two-sided parties the broker is dialling.
    pub heartbeat_tasks: u64,
    /// `--max-registrations`.
    pub registrations_limit: u64,
    /// Stores, one per client.
    pub stores: u64,
    /// Entries over every store.
    pub store_entries: u64,
    /// Accounted bytes over every store.
    pub store_bytes: u64,
    /// `--max-store-bytes`.
    pub store_bytes_limit: u64,
    /// Parties per rendezvous key, ascending by key.
    pub per_key: Vec<KeyRow>,
    /// Parties per advertised host, ascending by host.
    pub per_host: Vec<HostRow>,
}

fn count(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

impl Gauges {
    /// Read every gauge from the registry. `heartbeat_tasks` stays 0 here.
    pub fn of(registry: &Registry) -> Gauges {
        let mut g = Gauges {
            registrations_limit: count(registry.limits().max_registrations),
            store_bytes_limit: count(registry.limits().max_store_bytes),
            ..Gauges::default()
        };
        let mut keys: BTreeMap<Key, KeyRow> = BTreeMap::new();
        let mut hosts: BTreeMap<String, u64> = BTreeMap::new();
        for party in registry.parties() {
            let role = match party {
                Party::Service(_) => Role::Service,
                Party::Client(_) => Role::Client,
            };
            g.parties[role_index(role)][Mode::of(party.is_ping()) as usize] += 1;
            if party.failures() > 0 {
                g.failing += 1;
            }
            *hosts.entry(party.bind_addr().host.clone()).or_default() += 1;
            let row = keys.entry(party.key()).or_insert_with(|| KeyRow {
                key: party.key(),
                services: 0,
                unclaimed: 0,
                clients: 0,
            });
            match party {
                Party::Service(s) => {
                    row.services += 1;
                    if s.claimed_by.is_none() {
                        row.unclaimed += 1;
                        g.services_unclaimed += 1;
                    }
                }
                Party::Client(c) => {
                    row.clients += 1;
                    g.stores += 1;
                    g.store_entries += count(c.store.len());
                    g.store_bytes += count(c.store.bytes());
                }
            }
        }
        g.keys = count(keys.len());
        g.per_key = keys.into_values().collect();
        g.per_host = hosts
            .into_iter()
            .map(|(host, parties)| HostRow { host, parties })
            .collect();
        g
    }

    /// Registered parties of this role in this mode.
    pub fn parties(&self, role: Role, mode: Mode) -> u64 {
        self.parties[role_index(role)][mode as usize]
    }

    /// Registered parties of this role, both modes.
    pub fn parties_of(&self, role: Role) -> u64 {
        self.parties[role_index(role)].iter().sum()
    }

    /// Registered parties in this mode, both roles.
    pub fn parties_in(&self, mode: Mode) -> u64 {
        self.parties
            .iter()
            .map(|by_mode| by_mode[mode as usize])
            .sum()
    }
}

// ----- the JSON view -----------------------------------------------------------

/// The counters as JSON: maps keyed by the labels of section 2.1 of the
/// monitoring plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Totals {
    /// `nsm_requests_total`: kind, then outcome.
    pub requests: BTreeMap<String, BTreeMap<String, u64>>,
    /// `nsm_registrations_total`: by role.
    pub registrations: BTreeMap<String, u64>,
    /// `nsm_registrations_refused_total`: by reason.
    pub registrations_refused: BTreeMap<String, u64>,
    /// `nsm_removals_total`: role, then reason.
    pub removals: BTreeMap<String, BTreeMap<String, u64>>,
    /// `nsm_repairings_total`.
    pub repairings: u64,
    /// `nsm_heartbeats_total`: by outcome.
    pub heartbeats: BTreeMap<String, u64>,
    /// `nsm_heartbeat_duration_seconds`.
    pub heartbeat_seconds: HistogramView,
    /// `nsm_store_ops_total`: operation, then outcome.
    pub store_ops: BTreeMap<String, BTreeMap<String, u64>>,
}

impl Totals {
    /// Sum of a map of counts.
    pub fn sum(map: &BTreeMap<String, u64>) -> u64 {
        map.values().fold(0, |a, b| a.saturating_add(*b))
    }

    /// Sum of a nested map of counts.
    pub fn sum_nested(map: &BTreeMap<String, BTreeMap<String, u64>>) -> u64 {
        map.values()
            .map(Totals::sum)
            .fold(0, |a, b| a.saturating_add(b))
    }

    /// Requests with this outcome, over every kind.
    pub fn requests_with(&self, outcome: &str) -> u64 {
        self.requests
            .values()
            .filter_map(|by_outcome| by_outcome.get(outcome))
            .fold(0, |a, b| a.saturating_add(*b))
    }
}

/// A histogram as JSON: count, sum and cumulative bucket counts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistogramView {
    /// Observations.
    pub count: u64,
    /// Sum of every observation, in seconds.
    pub sum: f64,
    /// Observations at or below each bound, the last bound being `+Inf`.
    pub buckets: Vec<Bucket>,
}

/// One cumulative bucket of a [`HistogramView`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bucket {
    /// The upper bound, in seconds, as text (`"0.25"`, `"+Inf"`).
    pub le: String,
    /// Observations at or below it.
    pub count: u64,
}

/// The current counts as JSON.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    /// Registered services.
    pub services: u64,
    /// Of those, services no client holds.
    pub services_unclaimed: u64,
    /// Registered clients (each holds one store).
    pub clients: u64,
    /// Parties in ping mode, both roles.
    pub ping_parties: u64,
    /// Two-sided parties the broker is dialling.
    pub heartbeat_tasks: u64,
    /// Distinct rendezvous keys with at least one party.
    pub keys: u64,
    /// Parties with at least one consecutive failed heartbeat.
    pub failing: u64,
    /// Entries over every store.
    pub store_entries: u64,
    /// Accounted bytes over every store.
    pub store_bytes: u64,
}

impl From<&Gauges> for Counts {
    fn from(g: &Gauges) -> Self {
        Counts {
            services: g.parties_of(Role::Service),
            services_unclaimed: g.services_unclaimed,
            clients: g.parties_of(Role::Client),
            ping_parties: g.parties_in(Mode::Ping),
            heartbeat_tasks: g.heartbeat_tasks,
            keys: g.keys,
            failing: g.failing,
            store_entries: g.store_entries,
            store_bytes: g.store_bytes,
        }
    }
}

/// The limits in force, as JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LimitsView {
    /// `--max-frame-bytes`.
    pub max_frame_bytes: u64,
    /// `--max-connections`.
    pub max_connections: u64,
    /// `--max-registrations`.
    pub max_registrations: u64,
    /// `--max-store-bytes`.
    pub max_store_bytes: u64,
    /// `--max-registrations-per-host`.
    pub max_registrations_per_host: u64,
    /// `--require-matching-host`.
    pub require_matching_host: bool,
}

impl LimitsView {
    /// The limits and the policy together.
    pub fn new(limits: &Limits, policy: &BrokerPolicy) -> Self {
        LimitsView {
            max_frame_bytes: count(limits.max_frame_bytes),
            max_connections: count(limits.max_connections),
            max_registrations: count(limits.max_registrations),
            max_store_bytes: count(limits.max_store_bytes),
            max_registrations_per_host: count(policy.max_registrations_per_host),
            require_matching_host: policy.require_matching_host,
        }
    }
}

/// The timing in force, as JSON, in seconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimingView {
    /// `--heartbeat-interval`.
    pub heartbeat_interval: f64,
    /// `--heartbeat-timeout`.
    pub heartbeat_timeout: f64,
    /// `--fail-threshold`.
    pub fail_threshold: u32,
    /// `--ping-staleness`.
    pub ping_staleness: f64,
    /// `--request-timeout`.
    pub request_timeout: f64,
    /// `--connect-timeout`.
    pub connect_timeout: f64,
    /// How long a claim waits for a service to appear.
    pub claim_wait: f64,
}

impl From<&Timing> for TimingView {
    fn from(t: &Timing) -> Self {
        TimingView {
            heartbeat_interval: t.heartbeat_interval.as_secs_f64(),
            heartbeat_timeout: t.heartbeat_timeout.as_secs_f64(),
            fail_threshold: t.fail_threshold,
            ping_staleness: t.ping_staleness.as_secs_f64(),
            request_timeout: t.request_timeout.as_secs_f64(),
            connect_timeout: t.connect_timeout.as_secs_f64(),
            claim_wait: t.claim_wait.as_secs_f64(),
        }
    }
}

/// One registered party, as `GET /v1/status` and `nsm status --parties`
/// report it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartyRow {
    /// Broker-assigned id.
    pub id: PartyId,
    /// Service or client.
    pub role: Role,
    /// Rendezvous key.
    pub key: Key,
    /// Heartbeat endpoint.
    pub bind_addr: Addr,
    /// One-sided liveness.
    pub ping: bool,
    /// Consecutive failed heartbeats so far.
    pub failures: u32,
    /// For a service: the client holding it; for a client: its service.
    pub paired_with: Option<PartyId>,
    /// Seconds since the broker last heard from the party.
    pub last_seen_seconds_ago: u64,
}

impl PartyRow {
    /// Every party in the registry, services first, ascending by id, with
    /// `now` deciding how long ago each was last heard from.
    pub fn all(registry: &Registry, now: tokio::time::Instant) -> Vec<PartyRow> {
        registry
            .parties()
            .map(|party| {
                let (role, paired_with) = match party {
                    Party::Service(s) => (Role::Service, s.claimed_by),
                    Party::Client(c) => (Role::Client, Some(c.record.service)),
                };
                PartyRow {
                    id: party.id(),
                    role,
                    key: party.key(),
                    bind_addr: party.bind_addr().clone(),
                    ping: party.is_ping(),
                    failures: party.failures(),
                    paired_with,
                    last_seen_seconds_ago: now
                        .saturating_duration_since(party.last_seen())
                        .as_secs(),
                }
            })
            .collect()
    }
}

/// Everything about a running broker in one JSON document: the body of
/// `GET /v1/status` and the input of `nsm status`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Status {
    /// The binary's version.
    pub version: String,
    /// [`PROTOCOL_VERSION`].
    pub protocol_version: u16,
    /// When the broker started, in seconds since the Unix epoch.
    pub started_at: u64,
    /// Seconds since then.
    pub uptime_seconds: u64,
    /// The protocol listener's address, when known.
    pub bound: Option<Addr>,
    /// The limits and the admission policy in force.
    pub limits: LimitsView,
    /// The timing in force.
    pub timing: TimingView,
    /// The current counts.
    pub counts: Counts,
    /// Parties per rendezvous key.
    pub keys: Vec<KeyRow>,
    /// Parties per advertised host.
    pub hosts: Vec<HostRow>,
    /// The counters since start.
    pub totals: Totals,
    /// Every registered party.
    pub parties: Vec<PartyRow>,
}

impl Status {
    /// Assemble the view. `bound` is the listener's address when the
    /// caller knows it.
    pub fn new(
        metrics: &Metrics,
        gauges: &Gauges,
        parties: Vec<PartyRow>,
        limits: &Limits,
        policy: &BrokerPolicy,
        timing: &Timing,
        bound: Option<Addr>,
    ) -> Status {
        Status {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol_version: PROTOCOL_VERSION,
            started_at: metrics.started_at(),
            uptime_seconds: metrics.uptime().as_secs(),
            bound,
            limits: LimitsView::new(limits, policy),
            timing: TimingView::from(timing),
            counts: Counts::from(gauges),
            keys: gauges.per_key.clone(),
            hosts: gauges.per_host.clone(),
            totals: metrics.totals(),
            parties,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::Transport;
    use crate::protocol::{RegToken, StoreKey};

    fn token(n: u8) -> RegToken {
        RegToken::from_bytes([n; 16])
    }

    fn now() -> tokio::time::Instant {
        tokio::time::Instant::now()
    }

    fn registry_with_two_keys() -> Registry {
        let mut r = Registry::default();
        let s1 = r
            .publish(
                7,
                Addr::tcp("10.0.0.1", 9000),
                Addr::tcp("10.0.0.1", 7001),
                false,
                token(1),
                now(),
            )
            .unwrap();
        r.publish(
            7,
            Addr::tcp("10.0.0.2", 9000),
            Addr::tcp("10.0.0.2", 7002),
            true,
            token(2),
            now(),
        )
        .unwrap();
        r.publish(
            9,
            Addr::new(Transport::Https, "svc.example", 4433),
            Addr::new(Transport::Https, "svc.example", 4434),
            false,
            token(3),
            now(),
        )
        .unwrap();
        let (c, handle) = r
            .claim(7, Addr::tcp("10.0.0.1", 7003), false, token(4), now())
            .unwrap();
        assert_eq!(handle.id, s1);
        r.store(
            c,
            StoreOp::Put {
                key: "step".parse::<StoreKey>().unwrap(),
                value: "5".into(),
                if_version: None,
            },
        )
        .unwrap();
        r.record_failure(s1);
        r
    }

    #[test]
    fn gauges_describe_the_registry() {
        let r = registry_with_two_keys();
        let g = Gauges::of(&r);
        assert_eq!(g.parties(Role::Service, Mode::Heartbeat), 2);
        assert_eq!(g.parties(Role::Service, Mode::Ping), 1);
        assert_eq!(g.parties(Role::Client, Mode::Heartbeat), 1);
        assert_eq!(g.parties(Role::Client, Mode::Ping), 0);
        assert_eq!(g.parties_of(Role::Service), 3);
        assert_eq!(g.parties_in(Mode::Ping), 1);
        assert_eq!(g.services_unclaimed, 2);
        assert_eq!(g.keys, 2);
        assert_eq!(g.failing, 1);
        assert_eq!(g.heartbeat_tasks, 0);
        assert_eq!(g.registrations_limit, 10_000);
        assert_eq!(g.stores, 1);
        assert_eq!(g.store_entries, 1);
        assert!(g.store_bytes > 64, "{}", g.store_bytes);
        assert_eq!(g.store_bytes_limit, 16 * 1024);
        assert_eq!(
            g.per_key,
            vec![
                KeyRow {
                    key: 7,
                    services: 2,
                    unclaimed: 1,
                    clients: 1
                },
                KeyRow {
                    key: 9,
                    services: 1,
                    unclaimed: 1,
                    clients: 0
                },
            ]
        );
        assert_eq!(
            g.per_host,
            vec![
                HostRow {
                    host: "10.0.0.1".into(),
                    parties: 2
                },
                HostRow {
                    host: "10.0.0.2".into(),
                    parties: 1
                },
                HostRow {
                    host: "svc.example".into(),
                    parties: 1
                },
            ]
        );
        let counts = Counts::from(&g);
        assert_eq!(
            counts,
            Counts {
                services: 3,
                services_unclaimed: 2,
                clients: 1,
                ping_parties: 1,
                heartbeat_tasks: 0,
                keys: 2,
                failing: 1,
                store_entries: 1,
                store_bytes: g.store_bytes,
            }
        );
    }

    #[test]
    fn empty_registry_has_zero_gauges() {
        let g = Gauges::of(&Registry::default());
        assert_eq!(
            g,
            Gauges {
                registrations_limit: 10_000,
                store_bytes_limit: 16 * 1024,
                ..Gauges::default()
            }
        );
    }

    #[test]
    fn counters_count_what_they_are_told() {
        let m = Metrics::new();
        m.request(RequestKind::Publish, Outcome::Ok);
        m.request(RequestKind::Publish, Outcome::Ok);
        m.request(RequestKind::Claim, Outcome::Nack);
        m.request(RequestKind::Other, Outcome::Error);
        m.registered(Role::Service);
        m.registered(Role::Service);
        m.registered(Role::Client);
        m.refused(RefusalReason::NoService);
        m.removed(Role::Service, RemovalReason::HeartbeatsFailed);
        m.removed(Role::Client, RemovalReason::NoReplacement);
        m.repaired();
        m.heartbeat(HeartbeatOutcome::Ack, Duration::from_millis(3));
        m.heartbeat(HeartbeatOutcome::Fail, Duration::from_secs(3));
        m.store_op(StoreOpKind::Put, StoreOutcome::Applied);
        m.store_op(StoreOpKind::Put, StoreOutcome::NotApplied);
        m.store_op(StoreOpKind::Get, StoreOutcome::Refused);

        assert_eq!(m.requests(RequestKind::Publish, Outcome::Ok), 2);
        assert_eq!(m.requests(RequestKind::Claim, Outcome::Nack), 1);
        assert_eq!(m.requests(RequestKind::Claim, Outcome::Ok), 0);
        assert_eq!(m.requests(RequestKind::Other, Outcome::Error), 1);
        assert_eq!(m.registrations(Role::Service), 2);
        assert_eq!(m.registrations(Role::Client), 1);
        assert_eq!(m.refusals(RefusalReason::NoService), 1);
        assert_eq!(m.refusals(RefusalReason::Full), 0);
        assert_eq!(
            m.removals(Role::Service, RemovalReason::HeartbeatsFailed),
            1
        );
        assert_eq!(m.removals(Role::Client, RemovalReason::NoReplacement), 1);
        assert_eq!(m.removals(Role::Client, RemovalReason::NoPing), 0);
        assert_eq!(m.repairings(), 1);
        assert_eq!(m.heartbeats(HeartbeatOutcome::Ack), 1);
        assert_eq!(m.heartbeats(HeartbeatOutcome::Fail), 1);
        assert_eq!(m.heartbeat_seconds().count(), 2);
        assert!((m.heartbeat_seconds().sum_seconds() - 3.003).abs() < 1e-9);
        assert_eq!(m.store_ops(StoreOpKind::Put, StoreOutcome::Applied), 1);
        assert_eq!(m.store_ops(StoreOpKind::Put, StoreOutcome::NotApplied), 1);
        assert_eq!(m.store_ops(StoreOpKind::Get, StoreOutcome::Refused), 1);
        assert_eq!(m.store_ops(StoreOpKind::List, StoreOutcome::Applied), 0);

        let t = m.totals();
        assert_eq!(t.requests["publish"]["ok"], 2);
        assert_eq!(t.requests["claim"]["nack"], 1);
        assert_eq!(Totals::sum_nested(&t.requests), 4);
        assert_eq!(t.requests_with("ok"), 2);
        assert_eq!(t.requests_with("nack"), 1);
        assert_eq!(t.registrations["service"], 2);
        assert_eq!(Totals::sum(&t.registrations), 3);
        assert_eq!(t.registrations_refused["no_service"], 1);
        assert_eq!(t.removals["service"]["heartbeats_failed"], 1);
        assert_eq!(t.repairings, 1);
        assert_eq!(t.heartbeats["ack"], 1);
        assert_eq!(t.heartbeat_seconds.count, 2);
        assert_eq!(t.heartbeat_seconds.buckets.len(), 11);
        assert_eq!(t.heartbeat_seconds.buckets[0].le, "0.005");
        assert_eq!(t.heartbeat_seconds.buckets[0].count, 1);
        assert_eq!(t.heartbeat_seconds.buckets[10].le, "+Inf");
        assert_eq!(t.heartbeat_seconds.buckets[10].count, 2);
        assert_eq!(t.store_ops["put"]["not_applied"], 1);
    }

    #[test]
    fn histogram_buckets_are_cumulative_and_inclusive() {
        let h = Histogram::default();
        h.observe(Duration::from_millis(5)); // exactly the first bound: inclusive
        h.observe(Duration::from_millis(6));
        h.observe(Duration::from_millis(400));
        h.observe(Duration::from_secs(60)); // beyond the last bound
        assert_eq!(h.count(), 4);
        let c = h.cumulative();
        assert_eq!(c[0], ("0.005", 1));
        assert_eq!(c[1], ("0.01", 2));
        assert_eq!(c[5], ("0.25", 2));
        assert_eq!(c[6], ("0.5", 3));
        assert_eq!(c[9], ("5", 3));
        assert_eq!(c[10], ("+Inf", 4));
        assert!((h.sum_seconds() - 60.411).abs() < 1e-9);
    }

    #[test]
    fn exposition_has_every_family_once_and_the_right_shape() {
        let r = registry_with_two_keys();
        let m = Metrics::new();
        m.request(RequestKind::Publish, Outcome::Ok);
        m.heartbeat(HeartbeatOutcome::Ack, Duration::from_millis(12));
        let text = m.render(&Gauges::of(&r));

        let types: Vec<&str> = text
            .lines()
            .filter_map(|l| l.strip_prefix("# TYPE "))
            .collect();
        let mut names: Vec<&str> = types.iter().map(|t| t.split(' ').next().unwrap()).collect();
        let before = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), before, "a family is declared twice");
        for expected in [
            "nsm_build_info gauge",
            "nsm_start_time_seconds gauge",
            "nsm_parties gauge",
            "nsm_services_unclaimed gauge",
            "nsm_keys gauge",
            "nsm_parties_failing gauge",
            "nsm_heartbeat_tasks gauge",
            "nsm_registrations_limit gauge",
            "nsm_stores gauge",
            "nsm_store_entries gauge",
            "nsm_store_bytes gauge",
            "nsm_store_bytes_limit gauge",
            "nsm_requests_total counter",
            "nsm_registrations_total counter",
            "nsm_registrations_refused_total counter",
            "nsm_removals_total counter",
            "nsm_repairings_total counter",
            "nsm_heartbeats_total counter",
            "nsm_heartbeat_duration_seconds histogram",
            "nsm_store_ops_total counter",
        ] {
            assert!(types.contains(&expected), "missing `# TYPE {expected}`");
        }
        assert_eq!(types.len(), 20);

        // Every HELP precedes its TYPE, and every sample line parses as
        // `name{labels} value` with the name of a declared family.
        let mut declared: Vec<&str> = Vec::new();
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("# HELP ") {
                declared.push(rest.split(' ').next().unwrap());
            } else if let Some(rest) = line.strip_prefix("# TYPE ") {
                let name = rest.split(' ').next().unwrap();
                assert_eq!(declared.last(), Some(&name), "{line}");
            } else {
                let (sample, value) = line.rsplit_once(' ').expect(line);
                let name = sample.split('{').next().unwrap();
                let family = name
                    .strip_suffix("_bucket")
                    .or_else(|| name.strip_suffix("_sum"))
                    .or_else(|| name.strip_suffix("_count"))
                    .filter(|_| name.starts_with("nsm_heartbeat_duration_seconds"))
                    .unwrap_or(name);
                assert!(declared.contains(&family), "{line}");
                assert!(
                    value.parse::<f64>().is_ok(),
                    "value of `{line}` is not a number"
                );
            }
        }

        let has = |l: &str| assert!(text.lines().any(|x| x == l), "missing line `{l}`\n{text}");
        has(&format!(
            "nsm_build_info{{version=\"{}\",protocol_version=\"{PROTOCOL_VERSION}\"}} 1",
            env!("CARGO_PKG_VERSION")
        ));
        has("nsm_parties{role=\"service\",mode=\"heartbeat\"} 2");
        has("nsm_parties{role=\"service\",mode=\"ping\"} 1");
        has("nsm_parties{role=\"client\",mode=\"heartbeat\"} 1");
        has("nsm_parties{role=\"client\",mode=\"ping\"} 0");
        has("nsm_services_unclaimed 2");
        has("nsm_keys 2");
        has("nsm_parties_failing 1");
        has("nsm_stores 1");
        has("nsm_store_entries 1");
        has("nsm_registrations_limit 10000");
        has("nsm_store_bytes_limit 16384");
        has("nsm_requests_total{kind=\"publish\",outcome=\"ok\"} 1");
        has("nsm_requests_total{kind=\"store_relay\",outcome=\"error\"} 0");
        has("nsm_removals_total{role=\"client\",reason=\"no_replacement\"} 0");
        has("nsm_heartbeats_total{outcome=\"ack\"} 1");
        has("nsm_heartbeat_duration_seconds_bucket{le=\"0.01\"} 0");
        has("nsm_heartbeat_duration_seconds_bucket{le=\"0.025\"} 1");
        has("nsm_heartbeat_duration_seconds_bucket{le=\"+Inf\"} 1");
        has("nsm_heartbeat_duration_seconds_sum 0.012000");
        has("nsm_heartbeat_duration_seconds_count 1");
        has("nsm_store_ops_total{op=\"list\",outcome=\"applied\"} 0");
        assert!(text.ends_with('\n'));
        assert!(
            !text.contains("key=\"") && !text.contains("host=\"") && !text.contains("id=\""),
            "no per-key, per-host or per-party label (decision M3)"
        );
    }

    #[test]
    fn label_values_are_escaped() {
        assert_eq!(escape_label("plain"), "plain");
        assert_eq!(escape_label("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
        let mut e = Exposition::default();
        e.family("x", "help with \\ and\nnewline", "gauge");
        e.sample("x", &[("l", "v\"1")], 2);
        assert_eq!(
            e.out,
            "# HELP x help with \\\\ and\\nnewline\n# TYPE x gauge\nx{l=\"v\\\"1\"} 2\n"
        );
    }

    #[test]
    fn removal_reasons_have_log_text_and_labels() {
        assert_eq!(
            RemovalReason::HeartbeatsFailed.to_string(),
            "heartbeats failed"
        );
        assert_eq!(RemovalReason::NoPing.to_string(), "no ping received");
        assert_eq!(
            RemovalReason::NoReplacement.to_string(),
            "no replacement service"
        );
        assert_eq!(RemovalReason::NoPing.label(), "no_ping");
        assert_eq!(StoreOpKind::from(&StoreOp::List).label(), "list");
        assert_eq!(
            StoreOpKind::from(&StoreOp::Get {
                key: "k".parse().unwrap()
            })
            .label(),
            "get"
        );
        let applied = Stored {
            client: None,
            revision: 0,
            applied: true,
            entries: Vec::new(),
        };
        assert_eq!(StoreOutcome::of(&applied), StoreOutcome::Applied);
        assert_eq!(
            StoreOutcome::of(&Stored {
                applied: false,
                ..applied
            }),
            StoreOutcome::NotApplied
        );
    }

    #[test]
    fn status_round_trips_through_json() {
        let r = registry_with_two_keys();
        let m = Metrics::new();
        m.registered(Role::Service);
        let mut gauges = Gauges::of(&r);
        gauges.heartbeat_tasks = 3;
        let status = Status::new(
            &m,
            &gauges,
            PartyRow::all(&r, now()),
            r.limits(),
            &BrokerPolicy::default(),
            &Timing::default(),
            Some(Addr::tcp("127.0.0.1", 12000)),
        );
        assert_eq!(status.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(status.protocol_version, PROTOCOL_VERSION);
        assert!(status.started_at > 1_700_000_000);
        assert_eq!(status.counts.heartbeat_tasks, 3);
        assert_eq!(status.limits.max_registrations_per_host, 64);
        assert!(!status.limits.require_matching_host);
        assert!((status.timing.heartbeat_interval - 2.0).abs() < 1e-9);
        assert_eq!(status.timing.fail_threshold, 5);
        assert_eq!(status.parties.len(), 4);
        assert_eq!(status.parties[0].role, Role::Service);
        assert_eq!(status.parties[0].failures, 1);
        assert_eq!(status.parties[3].role, Role::Client);
        assert_eq!(status.parties[3].paired_with, Some(status.parties[0].id));
        assert_eq!(status.parties[0].paired_with, Some(status.parties[3].id));
        assert_eq!(status.keys.len(), 2);
        assert_eq!(status.hosts.len(), 3);
        assert_eq!(status.totals.registrations["service"], 1);

        let json = serde_json::to_string(&status).unwrap();
        let back: Status = serde_json::from_str(&json).unwrap();
        assert_eq!(back, status);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["bound"], "127.0.0.1:12000");
        assert_eq!(value["parties"][0]["role"], "service");
        assert_eq!(
            value["totals"]["heartbeat_seconds"]["buckets"][10]["le"],
            "+Inf"
        );
    }
}

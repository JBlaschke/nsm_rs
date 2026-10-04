//! Tests of the broker's admin listener over a running cluster: the metrics
//! and the status JSON follow what parties do, and the token guards them.

mod common;

use std::time::Duration;

use nsm::broker::Status;
use nsm::broker::admin::{AdminOpts, METRICS_CONTENT_TYPE, serve};
use nsm::broker::metrics::{HostRow, KeyRow};
use nsm::net::Transport;
use nsm::ops::{self, StoreKey, StoreOp};
use nsm::protocol::{Key, Role};
use tokio_util::sync::CancellationToken;

use common::Cluster;

const DEADLINE: Duration = Duration::from_secs(30);

async fn with_deadline<T>(f: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(DEADLINE, f)
        .await
        .expect("test exceeded its deadline")
}

async fn scrape(c: &Cluster) -> String {
    let resp = reqwest::get(format!("{}/metrics", c.admin_url()))
        .await
        .expect("scrape");
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some(METRICS_CONTENT_TYPE)
    );
    resp.text().await.expect("body")
}

async fn status(c: &Cluster) -> Status {
    let resp = reqwest::get(format!("{}/v1/status", c.admin_url()))
        .await
        .expect("status");
    assert_eq!(resp.status(), 200);
    resp.json().await.expect("status JSON")
}

/// The value of one series (`name{labels}` exactly as rendered) in a scrape.
fn sample(text: &str, series: &str) -> f64 {
    text.lines()
        .find_map(|l| l.strip_prefix(series)?.strip_prefix(' '))
        .unwrap_or_else(|| panic!("no series `{series}` in\n{text}"))
        .parse()
        .expect("a number")
}

/// Poll the scrape until `series` reaches `at_least`.
async fn wait_for_sample(c: &Cluster, series: &str, at_least: f64) -> String {
    loop {
        let text = scrape(c).await;
        if sample(&text, series) >= at_least {
            return text;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn metrics_and_status_follow_a_session() {
    for t in [Transport::Tcp, Transport::Http] {
        with_deadline(async {
            let c = Cluster::start(t).await;
            let fresh = scrape(&c).await;
            assert_eq!(
                sample(&fresh, "nsm_parties{role=\"service\",mode=\"heartbeat\"}"),
                0.0
            );
            assert_eq!(
                sample(
                    &fresh,
                    "nsm_requests_total{kind=\"publish\",outcome=\"ok\"}"
                ),
                0.0
            );
            assert_eq!(sample(&fresh, "nsm_registrations_limit"), 10_000.0);
            assert_eq!(sample(&fresh, "nsm_store_bytes_limit"), 16_384.0);
            assert!(sample(&fresh, "nsm_start_time_seconds") > 1.7e9);
            let before = status(&c).await;
            assert_eq!(before.bound, Some(c.broker_addr()));
            assert_eq!(before.counts.services, 0);
            assert!(before.parties.is_empty());
            assert_eq!(before.version, env!("CARGO_PKG_VERSION"));
            assert_eq!(before.protocol_version, nsm::protocol::PROTOCOL_VERSION);
            assert!(before.uptime_seconds < 60);

            let rendezvous: Key = "job-17/step.2".parse().unwrap();
            let service = c.publish(rendezvous.clone(), 9000).await;
            let client = c.claim(rendezvous.clone()).await;
            let other: Key = "job-18".parse().unwrap();
            assert!(c.try_claim(other).await.is_err(), "no service under job-18");
            ops::send(&client.bound(), "hi".into(), c.net())
                .await
                .unwrap();
            let key: StoreKey = "step".parse().unwrap();
            ops::store(
                &client.bound(),
                StoreOp::Put {
                    key: key.clone(),
                    value: "5".into(),
                    if_version: None,
                },
                c.net(),
            )
            .await
            .unwrap();
            let stale = ops::store(
                &service.bound(),
                StoreOp::Put {
                    key,
                    value: "6".into(),
                    if_version: Some(0),
                },
                c.net(),
            )
            .await
            .unwrap();
            assert!(!stale.applied);

            let text = wait_for_sample(&c, "nsm_heartbeats_total{outcome=\"ack\"}", 2.0).await;
            for (series, value) in [
                ("nsm_parties{role=\"service\",mode=\"heartbeat\"}", 1.0),
                ("nsm_parties{role=\"service\",mode=\"ping\"}", 0.0),
                ("nsm_parties{role=\"client\",mode=\"heartbeat\"}", 1.0),
                ("nsm_services_unclaimed", 0.0),
                ("nsm_keys", 1.0),
                ("nsm_heartbeat_tasks", 2.0),
                ("nsm_stores", 1.0),
                ("nsm_store_entries", 1.0),
                ("nsm_requests_total{kind=\"publish\",outcome=\"ok\"}", 1.0),
                ("nsm_requests_total{kind=\"claim\",outcome=\"ok\"}", 1.0),
                ("nsm_requests_total{kind=\"claim\",outcome=\"nack\"}", 1.0),
                ("nsm_requests_total{kind=\"deliver\",outcome=\"ok\"}", 1.0),
                (
                    "nsm_requests_total{kind=\"store_relay\",outcome=\"ok\"}",
                    2.0,
                ),
                ("nsm_registrations_total{role=\"service\"}", 1.0),
                ("nsm_registrations_total{role=\"client\"}", 1.0),
                (
                    "nsm_registrations_refused_total{reason=\"no_service\"}",
                    1.0,
                ),
                ("nsm_registrations_refused_total{reason=\"full\"}", 0.0),
                ("nsm_store_ops_total{op=\"put\",outcome=\"applied\"}", 1.0),
                (
                    "nsm_store_ops_total{op=\"put\",outcome=\"not_applied\"}",
                    1.0,
                ),
                (
                    "nsm_removals_total{role=\"service\",reason=\"heartbeats_failed\"}",
                    0.0,
                ),
            ] {
                assert_eq!(sample(&text, series), value, "{t:?}: {series}\n{text}");
            }
            // A heartbeat may time out now and then on a loaded machine; what
            // cannot happen while both parties live is a removal, so the
            // failures stay below the threshold.
            let failed = sample(&text, "nsm_heartbeats_total{outcome=\"fail\"}");
            assert!(failed < f64::from(c.timing().fail_threshold), "{text}");
            assert!(sample(&text, "nsm_store_bytes") > 64.0);
            assert!(sample(&text, "nsm_heartbeat_duration_seconds_count") >= 2.0);
            assert_eq!(
                sample(&text, "nsm_heartbeat_duration_seconds_count"),
                sample(&text, "nsm_heartbeats_total{outcome=\"ack\"}") + failed
            );
            assert_eq!(
                sample(&text, "nsm_heartbeat_duration_seconds_bucket{le=\"+Inf\"}"),
                sample(&text, "nsm_heartbeat_duration_seconds_count")
            );
            assert!(
                text.contains(&format!(
                    "nsm_build_info{{version=\"{}\",protocol_version=\"{}\"}} 1\n",
                    env!("CARGO_PKG_VERSION"),
                    nsm::protocol::PROTOCOL_VERSION
                )),
                "{text}"
            );

            let s = status(&c).await;
            assert_eq!(s.counts.services, 1);
            assert_eq!(s.counts.services_unclaimed, 0);
            assert_eq!(s.counts.clients, 1);
            assert_eq!(s.counts.ping_parties, 0);
            assert_eq!(s.counts.heartbeat_tasks, 2);
            assert_eq!(s.counts.keys, 1);
            assert!(s.counts.failing <= 2, "{s:?}");
            assert_eq!(s.counts.store_entries, 1);
            assert_eq!(
                s.keys,
                vec![KeyRow {
                    key: rendezvous.clone(),
                    services: 1,
                    unclaimed: 0,
                    clients: 1
                }]
            );
            assert_eq!(
                s.hosts,
                vec![HostRow {
                    host: "127.0.0.1".into(),
                    parties: 2
                }]
            );
            assert_eq!(s.parties.len(), 2);
            assert_eq!(s.parties[0].id, service.id());
            assert_eq!(s.parties[0].role, Role::Service);
            assert_eq!(s.parties[0].paired_with, Some(client.id()));
            assert_eq!(s.parties[0].bind_addr, service.bound());
            assert_eq!(s.parties[1].id, client.id());
            assert_eq!(s.parties[1].role, Role::Client);
            assert_eq!(s.parties[1].paired_with, Some(service.id()));
            assert!(s.parties.iter().all(|p| p.last_seen_seconds_ago < 30));
            assert_eq!(s.totals.requests["publish"]["ok"], 1);
            assert_eq!(s.totals.requests["store_relay"]["ok"], 2);
            assert_eq!(s.totals.registrations_refused["no_service"], 1);
            assert_eq!(s.totals.store_ops["put"]["not_applied"], 1);
            assert!(s.totals.heartbeats["ack"] >= 2);
            assert_eq!(
                s.totals.heartbeat_seconds.count,
                s.totals.heartbeats["ack"] + s.totals.heartbeats["fail"]
            );
            assert_eq!(s.limits.max_registrations, 10_000);
            assert!((s.timing.heartbeat_interval - 0.05).abs() < 1e-9);

            // The service dies; the client has no replacement and goes too.
            c.kill(service).await;
            c.wait_until(|snap| snap.is_empty()).await;
            let after = scrape(&c).await;
            for (series, value) in [
                ("nsm_parties{role=\"service\",mode=\"heartbeat\"}", 0.0),
                ("nsm_parties{role=\"client\",mode=\"heartbeat\"}", 0.0),
                ("nsm_stores", 0.0),
                ("nsm_heartbeat_tasks", 0.0),
                (
                    "nsm_removals_total{role=\"service\",reason=\"heartbeats_failed\"}",
                    1.0,
                ),
                (
                    "nsm_removals_total{role=\"client\",reason=\"no_replacement\"}",
                    1.0,
                ),
                ("nsm_repairings_total", 0.0),
            ] {
                assert_eq!(sample(&after, series), value, "{t:?}: {series}\n{after}");
            }
            assert!(
                sample(&after, "nsm_heartbeats_total{outcome=\"fail\"}")
                    >= f64::from(c.timing().fail_threshold)
            );
            let s = status(&c).await;
            assert_eq!(s.totals.removals["service"]["heartbeats_failed"], 1);
            assert_eq!(s.totals.removals["client"]["no_replacement"], 1);
            assert!(s.parties.is_empty());
            c.stop().await;
        })
        .await;
    }
}

#[tokio::test]
async fn repairings_and_ping_parties_are_counted() {
    with_deadline(async {
        let c = Cluster::start(Transport::Tcp).await;
        let s1 = c.publish(5, 9001).await;
        let s2 = c.publish_ping(5, 9002).await;
        // Ping-mode parties must ping, or the sweeper removes them.
        let _run_s2 = c.spawn_run(&s2);
        let client = c.claim_ping(5).await;
        let _run_client = c.spawn_run(&client);
        let text =
            wait_for_sample(&c, "nsm_requests_total{kind=\"ping\",outcome=\"ok\"}", 1.0).await;
        assert_eq!(
            sample(&text, "nsm_parties{role=\"service\",mode=\"ping\"}"),
            1.0
        );
        assert_eq!(
            sample(&text, "nsm_parties{role=\"client\",mode=\"ping\"}"),
            1.0
        );
        assert_eq!(sample(&text, "nsm_services_unclaimed"), 1.0);
        let s = status(&c).await;
        assert_eq!(s.counts.ping_parties, 2);
        assert_eq!(s.counts.heartbeat_tasks, 1);
        assert_eq!(s.counts.services_unclaimed, 1);

        c.kill(s1).await;
        let text = wait_for_sample(&c, "nsm_repairings_total", 1.0).await;
        assert_eq!(
            sample(
                &text,
                "nsm_removals_total{role=\"service\",reason=\"heartbeats_failed\"}"
            ),
            1.0
        );
        assert_eq!(sample(&text, "nsm_services_unclaimed"), 0.0);
        assert_eq!(
            sample(&text, "nsm_parties{role=\"client\",mode=\"ping\"}"),
            1.0
        );
        c.stop().await;
    })
    .await;
}

#[tokio::test]
async fn a_token_guards_every_route() {
    with_deadline(async {
        let c = Cluster::start(Transport::Tcp).await;
        let shutdown = CancellationToken::new();
        let guarded = serve(
            AdminOpts {
                bind: "127.0.0.1:0".parse().unwrap(),
                token: Some("adm1n".into()),
            },
            c.broker().clone(),
            c.broker_addr(),
            shutdown.clone(),
        )
        .await
        .unwrap();
        let base = format!("http://{}", guarded.local_addr());
        let http = reqwest::Client::new();
        for path in ["/healthz", "/metrics", "/v1/status"] {
            let anon = http.get(format!("{base}{path}")).send().await.unwrap();
            assert_eq!(anon.status(), 401, "{path}");
            assert_eq!(
                anon.json::<serde_json::Value>().await.unwrap()["error"],
                "missing or invalid bearer token"
            );
            let wrong = http
                .get(format!("{base}{path}"))
                .bearer_auth("nope")
                .send()
                .await
                .unwrap();
            assert_eq!(wrong.status(), 401, "{path}");
            let ok = http
                .get(format!("{base}{path}"))
                .bearer_auth("adm1n")
                .send()
                .await
                .unwrap();
            assert_eq!(ok.status(), 200, "{path}");
        }
        let s: Status = http
            .get(format!("{base}/v1/status"))
            .bearer_auth("adm1n")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(s.bound, Some(c.broker_addr()));
        shutdown.cancel();
        guarded.wait().await;
        c.stop().await;
    })
    .await;
}

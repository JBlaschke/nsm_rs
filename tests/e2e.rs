//! End-to-end tests: a broker, services and clients in one process, on
//! ephemeral loopback ports, over every transport.

mod common;

use std::time::Duration;

use nsm::config::BrokerPolicy;
use nsm::net::Transport;
use nsm::ops::{StoreKey, StoreOp, Stored};
use nsm::protocol::{Message, PartyId, RegToken, ServiceHandle};
use nsm::{Error, ops};

use common::{Cluster, Party, TRANSPORTS};

const DEADLINE: Duration = Duration::from_secs(20);

async fn with_deadline<T>(f: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(DEADLINE, f)
        .await
        .expect("test exceeded its deadline")
}

#[tokio::test]
async fn publish_then_claim_pairs_over_every_transport() {
    for &t in TRANSPORTS {
        with_deadline(async {
            let c = Cluster::start(t).await;
            let service = c.publish(42, 9000).await;
            let client = c.claim(42).await;
            let handle = client.service().expect("paired");
            assert_eq!(handle.id, service.id());
            assert_eq!(handle.service_port, 9000);
            assert!(
                serde_json::to_value(&handle).unwrap().get("key").is_none(),
                "a handle must not carry the rendezvous key"
            );
            assert_eq!(c.broker().snapshot().len(), 2, "{t:?}");
            c.stop().await;
        })
        .await;
    }
}

#[tokio::test]
async fn two_services_two_clients_are_paired_distinctly() {
    with_deadline(async {
        let c = Cluster::start(Transport::Tcp).await;
        let s1 = c.publish(7, 9001).await;
        let s2 = c.publish(7, 9002).await;
        let c1 = c.claim(7).await;
        let c2 = c.claim(7).await;
        let mut got = vec![
            c1.service().unwrap().service_port,
            c2.service().unwrap().service_port,
        ];
        got.sort_unstable();
        assert_eq!(got, vec![9001, 9002]);
        // A third client finds nothing free.
        let err = c.try_claim(7).await.unwrap_err();
        assert!(matches!(err, Error::Rejected(_)), "{err}");
        drop((s1, s2));
        c.stop().await;
    })
    .await;
}

#[tokio::test]
async fn claim_for_unknown_key_is_rejected() {
    with_deadline(async {
        let c = Cluster::start(Transport::Http).await;
        let err = c.try_claim(999).await.unwrap_err();
        assert!(
            matches!(err, Error::Rejected(ref r) if r.contains("999")),
            "{err}"
        );
        c.stop().await;
    })
    .await;
}

#[tokio::test]
async fn heartbeats_keep_parties_alive() {
    for &t in TRANSPORTS {
        with_deadline(async {
            let c = Cluster::start(t).await;
            let _service = c.publish(1, 9000).await;
            let _client = c.claim(1).await;
            tokio::time::sleep(c.timing().detection_window() * 2).await;
            let snap = c.broker().snapshot();
            assert_eq!(snap.len(), 2, "{t:?}: {snap:?}");
            assert!(snap.iter().all(|p| p.failures == 0), "{t:?}: {snap:?}");
            c.stop().await;
        })
        .await;
    }
}

#[tokio::test]
async fn dead_service_is_removed_and_its_client_repaired() {
    with_deadline(async {
        let c = Cluster::start(Transport::Tcp).await;
        let s1 = c.publish(5, 9001).await;
        let s2 = c.publish(5, 9002).await;
        let client = c.claim(5).await;
        let first = client.service().unwrap();
        assert_eq!(first.id, s1.id(), "lowest id first");
        let mut pairings = client.pairings();
        assert_eq!(pairings.borrow_and_update().clone(), Some(first.clone()));

        // Kill the first service without telling the broker.
        c.kill(s1).await;
        c.wait_until(|snap| !snap.iter().any(|p| p.id == first.id))
            .await;

        // The client learns its new service on the next heartbeat; the
        // pairing receiver wakes up with it.
        pairings.changed().await.unwrap();
        let repaired = pairings.borrow_and_update().clone();
        assert_eq!(repaired.map(|h| h.id), Some(s2.id()));
        assert_eq!(client.service().map(|h| h.id), Some(s2.id()));
        let collected = ops::collect(&client.bound(), c.net()).await.unwrap();
        assert!(
            matches!(&collected, ops::Collected::Client { service: Some(h), .. } if h.id == s2.id()),
            "{collected:?}"
        );
        c.stop().await;
    })
    .await;
}

#[tokio::test]
async fn client_without_replacement_is_removed_and_its_session_ends() {
    with_deadline(async {
        let c = Cluster::start(Transport::Tcp).await;
        let s = c.publish(6, 9001).await;
        let client = c.claim(6).await;
        let client_id = client.id();
        let run = tokio::spawn(client.into_session().run());
        c.kill(s).await;
        c.wait_until(|snap| snap.is_empty()).await;
        // Nobody heartbeats the client any more, so its watchdog fires.
        let outcome = run.await.unwrap();
        assert!(
            matches!(outcome, Err(Error::BrokerLost(_))),
            "{outcome:?} ({client_id})"
        );
        c.stop().await;
    })
    .await;
}

#[tokio::test]
async fn text_flows_both_ways_and_collect_reads_it() {
    for &t in TRANSPORTS {
        with_deadline(async {
            let c = Cluster::start(t).await;
            let service = c.publish(3, 9000).await;
            let client = c.claim(3).await;
            ops::send(&client.bound(), "job 17".into(), c.net())
                .await
                .unwrap();
            c.wait_until_true(|| service.state().inbox().is_some())
                .await;
            let got = ops::collect(&service.bound(), c.net()).await.unwrap();
            assert_eq!(
                got,
                ops::Collected::Service {
                    text: Some("job 17".into())
                },
                "{t:?}"
            );
            // And back: the service answers the client holding it.
            ops::send(&service.bound(), "ready".into(), c.net())
                .await
                .unwrap();
            c.wait_until_true(|| client.state().inbox().is_some()).await;
            let got = ops::collect(&client.bound(), c.net()).await.unwrap();
            assert_eq!(
                got,
                ops::Collected::Client {
                    service: client.service(),
                    text: Some("ready".into())
                },
                "{t:?}"
            );
            c.stop().await;
        })
        .await;
    }
}

#[tokio::test]
async fn send_to_an_unclaimed_service_is_refused() {
    with_deadline(async {
        let c = Cluster::start(Transport::Tcp).await;
        let service = c.publish(11, 9000).await;
        let err = ops::send(&service.bound(), "x".into(), c.net())
            .await
            .unwrap_err();
        assert!(
            matches!(&err, Error::Rejected(reason) if reason.contains("not claimed")),
            "{err}"
        );
        c.stop().await;
    })
    .await;
}

#[tokio::test]
async fn ping_mode_parties_stay_alive_and_receive_pending_items() {
    with_deadline(async {
        let c = Cluster::start(Transport::Http).await;
        let service = c.publish_ping(8, 9000).await;
        let client = c.claim_ping(8).await;
        let service_run = c.spawn_run(&service);
        let client_run = c.spawn_run(&client);
        tokio::time::sleep(c.timing().ping_staleness * 2).await;
        assert_eq!(c.broker().snapshot().len(), 2);
        ops::send(&client.bound(), "via ping".into(), c.net())
            .await
            .unwrap();
        c.wait_until_true(|| service.state().inbox().is_some())
            .await;
        assert_eq!(service.state().inbox().as_deref(), Some("via ping"));
        ops::send(&service.bound(), "back via ping".into(), c.net())
            .await
            .unwrap();
        c.wait_until_true(|| client.state().inbox().is_some()).await;
        assert_eq!(client.state().inbox().as_deref(), Some("back via ping"));
        c.stop().await;
        // With the broker gone the pinging parties give up.
        assert!(matches!(
            service_run.await.unwrap(),
            Err(Error::BrokerLost(_))
        ));
        assert!(matches!(
            client_run.await.unwrap(),
            Err(Error::BrokerLost(_))
        ));
    })
    .await;
}

#[tokio::test]
async fn silent_ping_party_is_swept() {
    with_deadline(async {
        let c = Cluster::start(Transport::Tcp).await;
        let service = c.publish_ping(9, 9000).await;
        // Never run the session, so no pings are sent.
        c.wait_until(|snap| snap.is_empty()).await;
        drop(service);
        c.stop().await;
    })
    .await;
}

#[tokio::test]
async fn broker_shutdown_ends_party_sessions_with_broker_lost() {
    with_deadline(async {
        let c = Cluster::start(Transport::Tcp).await;
        let service = c.publish(4, 9000).await;
        let run = c.spawn_run(&service);
        c.stop().await;
        assert!(matches!(run.await.unwrap(), Err(Error::BrokerLost(_))));
    })
    .await;
}

#[tokio::test]
async fn garbage_does_not_take_the_broker_down() {
    with_deadline(async {
        let c = Cluster::start(Transport::Tcp).await;
        let addr = c.broker_addr().resolve().await.unwrap();
        for _ in 0..3 {
            use tokio::io::AsyncWriteExt;
            let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
            let _ = s.write_all(&[0xff, 0xff, 0xff, 0xff, b'x']).await;
            drop(s);
            let s = tokio::net::TcpStream::connect(addr).await.unwrap();
            drop(s); // connect-and-close, the classic health probe
        }
        let service = c.publish(2, 9000).await;
        assert_eq!(service.id(), PartyId(1));
        c.stop().await;
    })
    .await;
}

#[tokio::test]
async fn per_host_registration_cap_is_enforced() {
    with_deadline(async {
        let c = Cluster::start_with(
            Transport::Tcp,
            BrokerPolicy {
                max_registrations_per_host: 2,
                ..BrokerPolicy::default()
            },
        )
        .await;
        let _s1 = c.publish(1, 9001).await;
        let _s2 = c.publish(1, 9002).await;
        let err = c.try_claim(1).await.unwrap_err();
        assert!(
            matches!(err, Error::Rejected(ref r) if r.contains("too many")),
            "{err}"
        );
        c.stop().await;
    })
    .await;
}

fn wrong_token() -> RegToken {
    RegToken::from_bytes([0xee; 16])
}

#[tokio::test]
async fn ping_and_deliver_require_the_registration_token() {
    with_deadline(async {
        let c = Cluster::start(Transport::Tcp).await;
        let raw = c.raw_client();
        let broker = c.broker_addr();
        let two_sided = c.publish(1, 9001).await;
        let pinger = c.publish_ping(2, 9002).await;
        let client = c.claim(1).await;
        assert_eq!(client.service().unwrap().id, two_sided.id());

        // Ping: wrong token and unknown id are indistinguishable refusals.
        let bad = raw
            .call(
                &broker,
                Message::Ping {
                    id: pinger.id(),
                    token: wrong_token(),
                },
            )
            .await
            .unwrap();
        let unknown = raw
            .call(
                &broker,
                Message::Ping {
                    id: PartyId(999),
                    token: wrong_token(),
                },
            )
            .await
            .unwrap();
        assert!(matches!(bad, Message::Nack { .. }), "{bad:?}");
        assert_eq!(bad, unknown);
        // A two-sided party cannot be pinged on, even with its own token.
        let reply = raw
            .call(
                &broker,
                Message::Ping {
                    id: two_sided.id(),
                    token: two_sided.token(),
                },
            )
            .await
            .unwrap();
        assert!(matches!(reply, Message::Nack { .. }), "{reply:?}");
        // The ping-mode party with its token gets a heartbeat carrying it.
        let reply = raw
            .call(
                &broker,
                Message::Ping {
                    id: pinger.id(),
                    token: pinger.token(),
                },
            )
            .await
            .unwrap();
        assert!(
            matches!(reply, Message::Heartbeat { token, .. } if token == pinger.token()),
            "{reply:?}"
        );

        // Deliver: needs the sender's own token and a peer to deliver to.
        let deliver = |from, token| Message::Deliver {
            from,
            token,
            text: "injected".into(),
        };
        for (from, token) in [
            (client.id(), wrong_token()),
            (pinger.id(), wrong_token()),
            // The right token, but nobody holds this service.
            (pinger.id(), pinger.token()),
        ] {
            let reply = raw.call(&broker, deliver(from, token)).await.unwrap();
            assert!(matches!(reply, Message::Nack { .. }), "{reply:?}");
        }
        // The paired client and the service it holds can both deliver.
        for (from, token) in [
            (client.id(), client.token()),
            (two_sided.id(), two_sided.token()),
        ] {
            assert_eq!(
                raw.call(&broker, deliver(from, token)).await.unwrap(),
                Message::Delivered,
                "{from}"
            );
        }
        c.stop().await;
    })
    .await;
}

#[tokio::test]
async fn forged_heartbeats_are_ignored_by_parties() {
    with_deadline(async {
        let c = Cluster::start(Transport::Tcp).await;
        let raw = c.raw_client();
        let service = c.publish(1, 9001).await;
        let client = c.claim(1).await;
        let paired = client.service().unwrap();
        let bogus = ServiceHandle {
            id: PartyId(999),
            host: "attacker".into(),
            service_port: 1,
        };
        let forged = Message::Heartbeat {
            token: wrong_token(),
            inbox: Some("planted".into()),
            service: Some(bogus.clone()),
        };
        let reply = raw.call(&client.bound(), forged.clone()).await.unwrap();
        assert!(matches!(reply, Message::Nack { .. }), "{reply:?}");
        assert_eq!(client.service(), Some(paired.clone()));
        let reply = raw.call(&service.bound(), forged).await.unwrap();
        assert!(matches!(reply, Message::Nack { .. }), "{reply:?}");
        assert_eq!(service.state().inbox(), None);
        // The real broker keeps working: a send still lands on the service.
        ops::send(&client.bound(), "genuine".into(), c.net())
            .await
            .unwrap();
        c.wait_until_true(|| service.state().inbox().is_some())
            .await;
        assert_eq!(service.state().inbox().as_deref(), Some("genuine"));
        assert_eq!(client.service(), Some(paired));
        c.stop().await;
    })
    .await;
}

#[tokio::test]
async fn control_plane_enforces_its_bearer_token() {
    with_deadline(async {
        // No Cluster here, so nothing else has installed the crypto provider
        // that reqwest needs; under nextest every test is its own process.
        nsm::tls::install_default_provider();
        let shutdown = tokio_util::sync::CancellationToken::new();
        let control = nsm::rest::serve(
            nsm::rest::ServeOpts {
                bind: "127.0.0.1:0".parse().unwrap(),
                token: Some("s3cret".into()),
                net: nsm::ops::NetOpts::default(),
            },
            shutdown.clone(),
        )
        .await
        .unwrap();
        let url = format!("http://{}/v1/jobs", control.local_addr());
        let http = reqwest::Client::new();
        assert_eq!(http.get(&url).send().await.unwrap().status(), 401);
        assert_eq!(
            http.get(&url)
                .bearer_auth("wrong")
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        assert_eq!(
            http.get(&url)
                .bearer_auth("s3cret")
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
        control.shutdown().await;
    })
    .await;
}

// ----- the shared store ------------------------------------------------------

fn store_key(text: &str) -> StoreKey {
    text.parse()
        .unwrap_or_else(|e| panic!("{text:?} is not a store key: {e}"))
}

fn get(key: &str) -> StoreOp {
    StoreOp::Get {
        key: store_key(key),
    }
}

fn put(key: &str, value: &str) -> StoreOp {
    StoreOp::Put {
        key: store_key(key),
        value: value.into(),
    }
}

fn delete(key: &str) -> StoreOp {
    StoreOp::Delete {
        key: store_key(key),
    }
}

/// Apply `op` through `party` and expect an answer.
async fn store(c: &Cluster, party: &Party, op: StoreOp) -> Stored {
    let kind = op.kind();
    ops::store(&party.bound(), op, c.net())
        .await
        .unwrap_or_else(|e| panic!("store {kind} through {}: {e}", party.id()))
}

/// The value and version of `key` in a reply, when it carries one.
fn value_of(stored: &Stored, key: &str) -> Option<(String, u64)> {
    stored
        .get(&store_key(key))
        .map(|e| (e.value.clone(), e.version))
}

#[tokio::test]
async fn store_is_shared_by_a_client_and_its_service() {
    for &t in TRANSPORTS {
        with_deadline(async {
            let c = Cluster::start(t).await;
            let service = c.publish(3, 9000).await;
            let client = c.claim(3).await;
            let owner = Some(client.id());

            // The client writes; the service reads the same entry.
            let written = store(&c, &client, put("step", "5")).await;
            assert_eq!(written.client, owner, "{t:?}");
            let (value, v1) = value_of(&written, "step").expect("the entry as written");
            assert_eq!(value, "5", "{t:?}");
            assert_eq!(written.revision, v1, "{t:?}");
            let read = store(&c, &service, get("step")).await;
            assert_eq!(read, written, "{t:?}: the service reads the client's write");

            // The service writes, text with a newline and quotes and an
            // empty value included; the client lists everything.
            let multiline = "/scratch/in.h5\n\"second line\"";
            let second = store(&c, &service, put("input/path", multiline)).await;
            assert_eq!(second.client, owner, "{t:?}: the service names its client");
            let (_, v2) = value_of(&second, "input/path").unwrap();
            let third = store(&c, &service, put("ready", "")).await;
            let (_, v3) = value_of(&third, "ready").unwrap();
            assert!(v1 < v2 && v2 < v3, "{t:?}: {v1} {v2} {v3}");
            let listed = store(&c, &client, StoreOp::List).await;
            assert_eq!(listed.client, owner, "{t:?}");
            assert_eq!(listed.revision, v3, "{t:?}");
            assert_eq!(
                listed.keys().map(|k| k.as_str()).collect::<Vec<_>>(),
                ["input/path", "ready", "step"],
                "{t:?}"
            );
            assert_eq!(
                value_of(&listed, "input/path"),
                Some((multiline.to_owned(), v2)),
                "{t:?}: the value arrives verbatim"
            );
            assert_eq!(value_of(&listed, "ready"), Some((String::new(), v3)));

            // Deleting at the service removes it for the client.
            let removed = store(&c, &service, delete("step")).await;
            assert_eq!(value_of(&removed, "step"), Some(("5".to_owned(), v1)));
            assert!(removed.revision > v3, "{t:?}: a delete is a write");
            let gone = store(&c, &client, get("step")).await;
            assert!(gone.entries.is_empty(), "{t:?}: {gone:?}");
            assert_eq!(gone.client, owner, "{t:?}");

            // A key never set: an answer with no entry, not a failure; a
            // delete of it succeeds and takes no version.
            let unset = store(&c, &client, get("missing")).await;
            assert!(unset.entries.is_empty(), "{t:?}: {unset:?}");
            assert_eq!(unset.client, owner, "{t:?}");
            let noop = store(&c, &service, delete("missing")).await;
            assert!(noop.entries.is_empty(), "{t:?}: {noop:?}");
            assert_eq!(noop.revision, removed.revision, "{t:?}");
            c.stop().await;
        })
        .await;
    }
}

#[tokio::test]
async fn store_survives_a_repairing() {
    with_deadline(async {
        let c = Cluster::start(Transport::Tcp).await;
        let s1 = c.publish(5, 9001).await;
        let s2 = c.publish(5, 9002).await;
        let client = c.claim(5).await;
        assert_eq!(client.service().map(|h| h.id), Some(s1.id()));
        let mut pairings = client.pairings();
        pairings.borrow_and_update();

        let by_client = store(&c, &client, put("from-client", "c")).await;
        let by_first = store(&c, &s1, put("from-first", "a")).await;
        let earlier = by_first.revision;
        assert!(by_client.revision < earlier);

        // The first service dies; the client is re-paired with the second.
        c.kill(s1).await;
        while pairings.borrow_and_update().as_ref().map(|h| h.id) != Some(s2.id()) {
            pairings.changed().await.unwrap();
        }

        // The replacement reads every earlier write, the dead service's
        // included, under the same client.
        let listed = store(&c, &s2, StoreOp::List).await;
        assert_eq!(listed.client, Some(client.id()));
        assert_eq!(listed.revision, earlier);
        assert_eq!(listed.entries, {
            let mut all = [by_client.entries, by_first.entries].concat();
            all.sort_by(|a, b| a.key.cmp(&b.key));
            all
        });
        // Its own write continues the numbering.
        let own = store(&c, &s2, put("from-second", "b")).await;
        let (_, version) = value_of(&own, "from-second").unwrap();
        assert!(
            listed.entries.iter().all(|e| e.version < version),
            "{version} after {listed:?}"
        );
        assert_eq!(own.client, Some(client.id()));
        let seen_by_client = store(&c, &client, get("from-second")).await;
        assert_eq!(seen_by_client.entries, own.entries);
        c.stop().await;
    })
    .await;
}

#[tokio::test]
async fn store_is_dropped_with_its_client_and_the_next_claim_starts_empty() {
    with_deadline(async {
        let c = Cluster::start(Transport::Tcp).await;
        let service = c.publish(6, 9000).await;
        let first = c.claim(6).await;
        let first_id = first.id();
        store(&c, &first, put("step", "5")).await;
        assert!(
            value_of(&store(&c, &service, get("step")).await, "step").is_some(),
            "the service reads its client's write"
        );

        // The client dies: its registration and its store go with it, and
        // the service is free again.
        c.kill(first).await;
        c.wait_until(|snap| {
            !snap.iter().any(|p| p.id == first_id)
                && snap
                    .iter()
                    .any(|p| p.id == service.id() && p.paired_with.is_none())
        })
        .await;
        let orphaned = store(&c, &service, StoreOp::List).await;
        assert_eq!(orphaned.client, None, "{orphaned:?}");
        assert!(orphaned.entries.is_empty(), "{orphaned:?}");

        // The next claim starts empty, under a new client id.
        let second = c.claim(6).await;
        assert_eq!(second.service().map(|h| h.id), Some(service.id()));
        assert_ne!(second.id(), first_id);
        let fresh = store(&c, &second, get("step")).await;
        assert!(fresh.entries.is_empty(), "{fresh:?}");
        assert_eq!(fresh.client, Some(second.id()));
        assert_eq!(fresh.revision, 0, "a store never written");
        let listed = store(&c, &service, StoreOp::List).await;
        assert_eq!(listed.client, Some(second.id()));
        assert!(listed.entries.is_empty(), "{listed:?}");
        c.stop().await;
    })
    .await;
}

#[tokio::test]
async fn unclaimed_service_reads_an_empty_store_and_may_not_write() {
    with_deadline(async {
        let c = Cluster::start(Transport::Tcp).await;
        let service = c.publish(11, 9000).await;
        let empty = Stored {
            client: None,
            revision: 0,
            entries: vec![],
        };
        assert_eq!(store(&c, &service, get("step")).await, empty);
        assert_eq!(store(&c, &service, StoreOp::List).await, empty);
        for op in [put("step", "5"), delete("step")] {
            let kind = op.kind();
            let err = ops::store(&service.bound(), op, c.net()).await.unwrap_err();
            assert!(
                matches!(&err, Error::Rejected(reason)
                    if *reason == format!("service {} is not claimed", service.id())),
                "{kind}: {err}"
            );
        }
        c.stop().await;
    })
    .await;
}

#[tokio::test]
async fn store_works_in_ping_mode() {
    with_deadline(async {
        let c = Cluster::start(Transport::Http).await;
        let service = c.publish_ping(8, 9000).await;
        let client = c.claim_ping(8).await;
        let service_run = c.spawn_run(&service);
        let client_run = c.spawn_run(&client);

        let written = store(&c, &client, put("step", "5")).await;
        assert_eq!(written.client, Some(client.id()));
        assert_eq!(store(&c, &service, get("step")).await, written);
        store(&c, &service, put("done", "yes")).await;
        let listed = store(&c, &client, StoreOp::List).await;
        assert_eq!(
            listed.keys().map(|k| k.as_str()).collect::<Vec<_>>(),
            ["done", "step"]
        );
        // Relays do not count as proof of life, but the pings do: both
        // parties are still registered after the staleness window.
        tokio::time::sleep(c.timing().ping_staleness * 2).await;
        assert_eq!(c.broker().snapshot().len(), 2);
        assert_eq!(store(&c, &service, StoreOp::List).await, listed);
        c.stop().await;
        service_run.abort();
        client_run.abort();
    })
    .await;
}

#[tokio::test]
async fn store_relay_requires_the_registration_token() {
    with_deadline(async {
        let c = Cluster::start(Transport::Tcp).await;
        let raw = c.raw_client();
        let broker = c.broker_addr();
        let service = c.publish(1, 9001).await;
        let client = c.claim(1).await;
        let relay = |from, token, op| Message::StoreRelay { from, token, op };
        let refused = Message::nack("unknown party or wrong token");

        // Wrong token and unknown id: the same refusal.
        let wrong = raw
            .call(&broker, relay(client.id(), wrong_token(), put("k", "v")))
            .await
            .unwrap();
        let unknown = raw
            .call(&broker, relay(PartyId(999), wrong_token(), StoreOp::List))
            .await
            .unwrap();
        assert_eq!(wrong, refused);
        assert_eq!(unknown, refused);
        // Another party's token does not work either.
        let borrowed = raw
            .call(&broker, relay(service.id(), client.token(), StoreOp::List))
            .await
            .unwrap();
        assert_eq!(borrowed, refused);
        // `store` carries no credentials, so the broker does not take it.
        let bare = raw
            .call(&broker, Message::Store { op: StoreOp::List })
            .await
            .unwrap();
        assert_eq!(bare, Message::nack("unexpected store at the broker"));
        // Nothing was written by any of that.
        let listed = store(&c, &client, StoreOp::List).await;
        assert!(listed.entries.is_empty(), "{listed:?}");

        // With its own token each party reaches the store.
        for (from, token) in [
            (client.id(), client.token()),
            (service.id(), service.token()),
        ] {
            match raw
                .call(&broker, relay(from, token, StoreOp::List))
                .await
                .unwrap()
            {
                Message::Stored(stored) => assert_eq!(stored.client, Some(client.id()), "{from}"),
                other => panic!("{from}: {other:?}"),
            }
        }

        // A removed service's id and token no longer verify.
        let doomed = c.publish(2, 9002).await;
        let (doomed_id, doomed_token) = (doomed.id(), doomed.token());
        c.kill(doomed).await;
        c.wait_until(|snap| !snap.iter().any(|p| p.id == doomed_id))
            .await;
        let reply = raw
            .call(&broker, relay(doomed_id, doomed_token, StoreOp::List))
            .await
            .unwrap();
        assert_eq!(reply, refused);
        c.stop().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_writes_from_both_sides_are_serialised() {
    const PER_SIDE: usize = 20;
    with_deadline(async {
        let c = Cluster::start(Transport::Tcp).await;
        let service = c.publish(4, 9000).await;
        let client = c.claim(4).await;
        let mut writers = Vec::with_capacity(2 * PER_SIDE);
        for i in 0..PER_SIDE {
            for (side, party) in [("client", &client), ("service", &service)] {
                let addr = party.bound();
                let net = c.net().clone();
                let value = format!("{side}-{i}");
                writers.push(tokio::spawn(async move {
                    let reply = ops::store(&addr, put("shared", &value), &net).await;
                    (value, reply)
                }));
            }
        }
        let mut versions = Vec::with_capacity(writers.len());
        for writer in writers {
            let (value, reply) = writer.await.unwrap();
            let stored = reply.unwrap_or_else(|e| panic!("{value}: {e}"));
            let (written, version) = value_of(&stored, "shared").expect("the entry as written");
            assert_eq!(written, value);
            versions.push((version, value));
        }
        versions.sort();
        versions.dedup_by_key(|(version, _)| *version);
        assert_eq!(
            versions.len(),
            2 * PER_SIDE,
            "every write has its own version"
        );

        // The last write in the broker's order is the one that stayed.
        let (highest, last_value) = versions.last().cloned().unwrap();
        let read = store(&c, &client, get("shared")).await;
        assert_eq!(value_of(&read, "shared"), Some((last_value, highest)));
        assert_eq!(read.revision, highest);
        c.stop().await;
    })
    .await;
}

#[tokio::test]
async fn a_full_store_refuses_a_write_and_keeps_its_contents() {
    for &t in TRANSPORTS {
        with_deadline(async {
            let c = Cluster::start(t).await;
            let service = c.publish(7, 9000).await;
            let client = c.claim(7).await;
            // Two values of 10 KiB each do not fit the default 16 KiB budget.
            let first = "a".repeat(10 * 1024);
            let written = store(&c, &client, put("first", &first)).await;
            let err = ops::store(
                &service.bound(),
                put("second", &"b".repeat(10 * 1024)),
                c.net(),
            )
            .await
            .unwrap_err();
            assert!(
                matches!(&err, Error::Rejected(reason) if reason.starts_with("store full: ")),
                "{t:?}: {err}"
            );
            let listed = store(&c, &service, StoreOp::List).await;
            assert_eq!(listed.entries, written.entries, "{t:?}");
            assert_eq!(listed.revision, written.revision, "{t:?}");
            // A smaller value in the same entry always fits.
            store(&c, &service, put("first", "short")).await;
            store(&c, &client, put("second", "fits now")).await;
            c.stop().await;
        })
        .await;
    }
}

#[tokio::test]
async fn a_write_acknowledged_before_a_send_is_visible_once_the_text_arrives() {
    with_deadline(async {
        let c = Cluster::start(Transport::Tcp).await;
        let service = c.publish(9, 9000).await;
        let client = c.claim(9).await;
        for (writer, reader, text) in [(&client, &service, "go"), (&service, &client, "done")] {
            let written = store(&c, writer, put(text, "payload")).await;
            ops::send(&writer.bound(), text.into(), c.net())
                .await
                .unwrap();
            // The reader learns about the text the way an operator does, by
            // polling collect; by then the write is there.
            loop {
                let collected = ops::collect(&reader.bound(), c.net()).await.unwrap();
                if collected.text().as_deref() == Some(text) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let read = store(&c, reader, get(text)).await;
            assert_eq!(read.entries, written.entries, "{text}");
        }
        c.stop().await;
    })
    .await;
}

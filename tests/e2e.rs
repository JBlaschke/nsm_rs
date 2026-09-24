//! End-to-end tests: a broker, services and clients in one process, on
//! ephemeral loopback ports, over every transport.

mod common;

use std::time::Duration;

use nsm::config::BrokerPolicy;
use nsm::net::Transport;
use nsm::protocol::{Message, PartyId, RegToken, ServiceHandle};
use nsm::{ops, Error};

use common::{Cluster, TRANSPORTS};

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

        // Kill the first service without telling the broker.
        c.kill(s1).await;
        c.wait_until(|snap| !snap.iter().any(|p| p.id == first.id))
            .await;

        // The client learns its new service on the next heartbeat.
        c.wait_until_true(|| client.service().map(|h| h.id) == Some(s2.id()))
            .await;
        let collected = ops::collect(&client.bound(), c.net()).await.unwrap();
        assert_eq!(collected.service.unwrap().id, s2.id());
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
async fn send_reaches_the_service_and_collect_reads_it() {
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
            assert_eq!(got.text.as_deref(), Some("job 17"), "{t:?}");
            // Sending to a service is refused.
            let err = ops::send(&service.bound(), "x".into(), c.net())
                .await
                .unwrap_err();
            assert!(matches!(err, Error::Rejected(_)), "{err}");
            c.stop().await;
        })
        .await;
    }
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

        // Deliver: needs the paired client's token and the right target.
        let deliver = |from, token, to| Message::Deliver {
            from,
            token,
            to,
            text: "injected".into(),
        };
        for (from, token, to) in [
            (client.id(), wrong_token(), two_sided.id()),
            (client.id(), client.token(), pinger.id()),
            (two_sided.id(), two_sided.token(), two_sided.id()),
        ] {
            let reply = raw.call(&broker, deliver(from, token, to)).await.unwrap();
            assert!(matches!(reply, Message::Nack { .. }), "{reply:?}");
        }
        assert_eq!(
            raw.call(
                &broker,
                deliver(client.id(), client.token(), two_sided.id())
            )
            .await
            .unwrap(),
            Message::Delivered
        );
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

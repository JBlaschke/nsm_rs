//! Load test: many parties on one broker. Ignored by default because it
//! takes a while; run it with `cargo test --test stress -- --ignored`.

mod common;

use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

use nsm::config::BrokerPolicy;
use nsm::net::Transport;
use nsm::ops;
use nsm::protocol::PartyId;

use common::{Cluster, Party};

const PARTIES: usize = 50;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "stress test; run with --ignored"]
async fn fifty_services_and_fifty_clients_pair_exchange_and_survive_churn() {
    tokio::time::timeout(Duration::from_secs(180), async {
        // Everybody lives on 127.0.0.1 here, so lift the per-host cap.
        let c = Cluster::start_with(
            Transport::Tcp,
            BrokerPolicy {
                max_registrations_per_host: 4 * PARTIES,
                ..BrokerPolicy::default()
            },
        )
        .await;

        let mut services: Vec<Party> = Vec::with_capacity(PARTIES);
        for i in 0..PARTIES {
            services.push(c.publish(1, 9000 + i as u16).await);
        }
        let mut clients: Vec<Party> = Vec::with_capacity(PARTIES);
        for _ in 0..PARTIES {
            clients.push(c.claim(1).await);
        }
        let paired: BTreeSet<PartyId> = clients
            .iter()
            .map(|cl| cl.service().expect("paired").id)
            .collect();
        assert_eq!(
            paired.len(),
            PARTIES,
            "every client holds a distinct service"
        );
        assert!(c.try_claim(1).await.is_err(), "nothing is left to claim");

        // Every client sends its own text; every paired service receives it.
        for (i, cl) in clients.iter().enumerate() {
            ops::send(&cl.bound(), format!("job {i}"), c.net())
                .await
                .unwrap();
        }
        let by_id: HashMap<PartyId, &Party> = services.iter().map(|s| (s.id(), s)).collect();
        for (i, cl) in clients.iter().enumerate() {
            let svc = by_id[&cl.service().unwrap().id];
            c.wait_until_true(|| svc.state().inbox().is_some()).await;
            assert_eq!(svc.state().inbox(), Some(format!("job {i}")));
        }

        // Everybody survives a few heartbeat rounds.
        tokio::time::sleep(c.timing().heartbeat_interval * 10).await;
        let snap = c.broker().snapshot();
        assert_eq!(snap.len(), 2 * PARTIES);
        assert!(snap.iter().all(|p| p.failures == 0), "{snap:?}");

        // Ten services die: with no spare service their clients are removed.
        let dead: Vec<PartyId> = services.iter().take(10).map(Party::id).collect();
        for s in services.drain(..10) {
            c.kill(s).await;
        }
        c.wait_until(|snap| snap.len() == 2 * PARTIES - 20).await;
        let snap = c.broker().snapshot();
        assert!(
            snap.iter().all(|p| !dead.contains(&p.id)
                && p.paired_with.is_some_and(|other| !dead.contains(&other)))
        );

        // Ten surviving clients leave: their services are freed and can be
        // claimed again.
        let alive: BTreeSet<PartyId> = snap.iter().map(|p| p.id).collect();
        let mut leaving = Vec::new();
        let mut i = 0;
        while leaving.len() < 10 && i < clients.len() {
            if alive.contains(&clients[i].id()) {
                leaving.push(clients.remove(i));
            } else {
                i += 1;
            }
        }
        assert_eq!(leaving.len(), 10);
        let freed: Vec<PartyId> = leaving.iter().map(|cl| cl.service().unwrap().id).collect();
        for cl in leaving {
            c.kill(cl).await;
        }
        c.wait_until(|snap| {
            snap.len() == 2 * PARTIES - 30
                && freed
                    .iter()
                    .all(|s| snap.iter().any(|p| p.id == *s && p.paired_with.is_none()))
        })
        .await;
        for _ in 0..10 {
            let cl = c.claim(1).await;
            assert!(freed.contains(&cl.service().unwrap().id));
            clients.push(cl);
        }
        c.wait_until(|snap| snap.len() == 2 * PARTIES - 20).await;
        c.stop().await;
    })
    .await
    .expect("stress test exceeded its deadline");
}

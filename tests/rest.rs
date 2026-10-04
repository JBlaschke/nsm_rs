//! Integration tests for the REST control plane (`nsm serve`): every route,
//! the job lifecycle, error mapping and the bearer token, with a broker
//! running in the same process.

mod common;

use std::time::Duration;

use nsm::net::{Addr, Transport};
use nsm::ops::NetOpts;
use nsm::protocol::Key;
use nsm::rest::{ControlPlane, ServeOpts, serve};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use common::Cluster;

const DEADLINE: Duration = Duration::from_secs(30);

async fn with_deadline<T>(f: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(DEADLINE, f)
        .await
        .expect("test exceeded its deadline")
}

/// A control plane on an ephemeral loopback port plus an HTTP client that
/// presents the token, when there is one.
struct Api {
    base: String,
    http: reqwest::Client,
    token: Option<String>,
    control: Option<ControlPlane>,
}

impl Api {
    async fn start(net: NetOpts, token: Option<&str>) -> Api {
        nsm::tls::install_default_provider();
        let control = serve(
            ServeOpts {
                bind: "127.0.0.1:0".parse().unwrap(),
                token: token.map(str::to_owned),
                net,
            },
            CancellationToken::new(),
        )
        .await
        .expect("control plane starts");
        Api {
            base: format!("http://{}", control.local_addr()),
            http: reqwest::Client::new(),
            token: token.map(str::to_owned),
            control: Some(control),
        }
    }

    fn bare(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        self.http.request(method, format!("{}{path}", self.base))
    }

    fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        let req = self.bare(method, path);
        match &self.token {
            Some(t) => req.bearer_auth(t),
            None => req,
        }
    }

    /// Status and body; a non-JSON body (axum's own rejections) comes back
    /// as a JSON string.
    async fn send(&self, req: reqwest::RequestBuilder) -> (StatusCode, Value) {
        let resp = req.send().await.expect("request completes");
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let body = serde_json::from_str(&text).unwrap_or(Value::String(text));
        (status, body)
    }

    async fn get(&self, path: &str) -> (StatusCode, Value) {
        self.send(self.request(Method::GET, path)).await
    }

    async fn post(&self, path: &str, body: &Value) -> (StatusCode, Value) {
        self.send(self.request(Method::POST, path).json(body)).await
    }

    async fn post_raw(&self, path: &str, body: &'static str) -> (StatusCode, Value) {
        self.send(self.request(Method::POST, path).body(body)).await
    }

    async fn delete(&self, path: &str) -> (StatusCode, Value) {
        self.send(self.request(Method::DELETE, path)).await
    }

    async fn stop(mut self) {
        if let Some(control) = self.control.take() {
            control.shutdown().await;
        }
    }
}

/// A publish/claim body selecting the loopback address, plus `extra` fields.
fn party_body(broker: &Addr, key: impl Into<Value>, extra: Value) -> Value {
    let mut body = json!({
        "broker": broker.to_string(),
        "key": key.into(),
        "ip_start": "127.",
        "ip_version": "4",
    });
    if let (Some(dst), Some(src)) = (body.as_object_mut(), extra.as_object()) {
        for (k, v) in src {
            dst.insert(k.clone(), v.clone());
        }
    }
    body
}

fn unused_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[tokio::test]
async fn health_interfaces_and_ips() {
    with_deadline(async {
        let api = Api::start(NetOpts::default(), None).await;

        let (status, body) = api.get("/healthz").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], json!(true));

        let (status, body) = api.get("/v1/interfaces").await;
        assert_eq!(status, StatusCode::OK);
        let names = body["interfaces"].as_array().expect("array");
        assert!(
            !names.is_empty() && names.iter().all(Value::is_string),
            "{body}"
        );
        let (status, body) = api.get("/v1/interfaces?ip_version=4").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["interfaces"].is_array());

        let (status, body) = api.get("/v1/ips?ip_start=127.&ip_version=4").await;
        assert_eq!(status, StatusCode::OK);
        let rows = body["addresses"].as_array().expect("array");
        assert!(rows.iter().any(|r| r["ip"] == json!("127.0.0.1")), "{body}");
        assert!(rows.iter().all(|r| r["interface"].is_string()), "{body}");

        // A filter that matches nothing is an empty list, not an error.
        let (status, body) = api.get("/v1/ips?interface=no-such-interface0").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["addresses"], json!([]));

        // Bad query values, unknown routes and wrong methods.
        assert_eq!(
            api.get("/v1/ips?ip_version=7").await.0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(api.get("/nope").await.0, StatusCode::NOT_FOUND);
        assert_eq!(
            api.post("/healthz", &json!({})).await.0,
            StatusCode::METHOD_NOT_ALLOWED
        );
        api.stop().await;
    })
    .await;
}

#[tokio::test]
async fn publish_claim_send_collect_and_cancel_through_the_api() {
    with_deadline(async {
        let cluster = Cluster::start(Transport::Http).await;
        let api = Api::start(cluster.net().clone(), None).await;
        let broker = cluster.broker_addr();

        let (status, publish) = api
            .post(
                "/v1/publish",
                &party_body(&broker, 7, json!({ "service_port": 9100 })),
            )
            .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{publish}");
        assert_eq!(publish["id"], json!(1));
        assert_eq!(publish["kind"], json!("publish"));
        assert_eq!(publish["state"], json!("running"));
        assert!(publish["party_id"].is_u64(), "{publish}");
        // An integer key is accepted as its decimal text and shown as text.
        assert_eq!(publish["key"], json!("7"));
        assert_eq!(publish["error"], Value::Null);
        assert_eq!(publish["service"], Value::Null);
        assert_eq!(publish["broker"], json!(broker.to_string()));
        let service_hb = publish["bind_addr"].as_str().expect("bind_addr").to_owned();
        assert!(service_hb.starts_with("http://127.0.0.1:"), "{service_hb}");

        let (status, claim) = api
            .post("/v1/claim", &party_body(&broker, 7, json!({})))
            .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{claim}");
        assert_eq!(claim["id"], json!(2));
        assert_eq!(claim["kind"], json!("claim"));
        assert_eq!(claim["service"]["service_port"], json!(9100));
        assert_eq!(claim["service"]["host"], json!("127.0.0.1"));
        assert_eq!(claim["service"]["id"], publish["party_id"]);
        assert!(
            claim["service"].get("key").is_none(),
            "handles never carry the key: {claim}"
        );
        let client_hb = claim["bind_addr"].as_str().expect("bind_addr").to_owned();

        let (status, jobs) = api.get("/v1/jobs").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(jobs.as_array().map(Vec::len), Some(2), "{jobs}");
        let (status, job) = api.get("/v1/jobs/1").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(job["kind"], json!("publish"));

        let (status, sent) = api
            .post("/v1/send", &json!({ "party": client_hb, "msg": "job 17" }))
            .await;
        assert_eq!(status, StatusCode::OK, "{sent}");
        assert_eq!(sent, json!({ "delivered": true }));

        // Delivery rides on the service's next heartbeat.
        let text = loop {
            let (status, collected) = api
                .post("/v1/collect", &json!({ "party": service_hb }))
                .await;
            assert_eq!(status, StatusCode::OK, "{collected}");
            assert_eq!(collected["role"], json!("service"), "{collected}");
            assert!(collected.get("service").is_none(), "{collected}");
            if let Some(text) = collected["text"].as_str() {
                break text.to_owned();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert_eq!(text, "job 17");
        let (status, collected) = api
            .post("/v1/collect", &json!({ "party": client_hb }))
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(collected["role"], json!("client"), "{collected}");
        assert_eq!(collected["text"], Value::Null, "{collected}");
        assert_eq!(collected["service"]["service_port"], json!(9100));

        // And back: the service answers the client holding it.
        let (status, sent) = api
            .post("/v1/send", &json!({ "party": service_hb, "msg": "ready" }))
            .await;
        assert_eq!(status, StatusCode::OK, "{sent}");
        let text = loop {
            let (status, collected) = api
                .post("/v1/collect", &json!({ "party": client_hb }))
                .await;
            assert_eq!(status, StatusCode::OK, "{collected}");
            assert_eq!(collected["role"], json!("client"), "{collected}");
            assert_eq!(collected["service"]["service_port"], json!(9100));
            if let Some(text) = collected["text"].as_str() {
                break text.to_owned();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert_eq!(text, "ready");

        // Cancelling the client keeps the job (as cancelled) and, once the
        // broker notices, frees the service.
        let (status, cancelled) = api.delete("/v1/jobs/2").await;
        assert_eq!(status, StatusCode::OK, "{cancelled}");
        assert_eq!(cancelled["state"], json!("cancelled"));
        let (status, again) = api.get("/v1/jobs/2").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(again["state"], json!("cancelled"));
        assert_eq!(api.delete("/v1/jobs/42").await.0, StatusCode::NOT_FOUND);
        cluster
            .wait_until(|snap| snap.len() == 1 && snap[0].paired_with.is_none())
            .await;

        // Stopping the control plane ends its remaining jobs; the broker
        // notices the vanished service.
        api.stop().await;
        cluster.wait_until(|snap| snap.is_empty()).await;
        cluster.stop().await;
    })
    .await;
}

#[tokio::test]
async fn errors_map_to_statuses() {
    with_deadline(async {
        let cluster = Cluster::start(Transport::Http).await;
        let api = Api::start(cluster.net().clone(), None).await;
        let broker = cluster.broker_addr();

        // Malformed, incomplete or oversized bodies are the caller's fault.
        let (status, body) = api.post_raw("/v1/send", "not json").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"].is_string(), "{body}");
        assert_eq!(
            api.post("/v1/publish", &json!({})).await.0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            api.post("/v1/claim", &json!({ "broker": "ftp://x:1", "key": 1 }))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
        let huge = json!({ "party": "127.0.0.1:1", "msg": "x".repeat(70 * 1024) });
        assert_eq!(
            api.post("/v1/send", &huge).await.0,
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(api.get("/v1/jobs/abc").await.0, StatusCode::BAD_REQUEST);
        let (status, body) = api.get("/v1/jobs/42").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(
            body["error"].as_str().unwrap_or("").contains("42"),
            "{body}"
        );

        // A key against the rule is the caller's fault too.
        let (status, body) = api
            .post("/v1/claim", &party_body(&broker, "a b", json!({})))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap_or("")
                .contains("rendezvous key"),
            "{body}"
        );

        // The broker's refusal is reported as a 400 that says why.
        let (status, body) = api
            .post("/v1/claim", &party_body(&broker, 999, json!({})))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(
            body["error"].as_str().unwrap_or("").contains("999"),
            "{body}"
        );

        // An unreachable broker or party is upstream trouble: a 502 with a
        // message, never a crash, and no job is left behind.
        let dead = Addr::new(Transport::Http, "127.0.0.1", unused_port());
        let (status, body) = api
            .post(
                "/v1/publish",
                &party_body(&dead, 1, json!({ "service_port": 9000 })),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
        assert!(body["error"].is_string(), "{body}");
        let (status, body) = api
            .post("/v1/collect", &json!({ "party": dead.to_string() }))
            .await;
        assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
        assert_eq!(api.get("/v1/jobs").await.1, json!([]));
        assert_eq!(api.get("/healthz").await.0, StatusCode::OK);

        api.stop().await;
        cluster.stop().await;
    })
    .await;
}

#[tokio::test]
async fn store_through_the_api() {
    with_deadline(async {
        let cluster = Cluster::start(Transport::Http).await;
        let api = Api::start(cluster.net().clone(), None).await;
        let service = cluster.publish(7, 9100).await;
        let client = cluster.claim(7).await;
        let (client_hb, service_hb) = (client.bound().to_string(), service.bound().to_string());
        let owner = json!(client.id());

        // A put at the client answers with the entry as written.
        let (status, put) = api
            .post(
                "/v1/store",
                &json!({ "party": client_hb, "op": "put", "key": "step", "value": "5" }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{put}");
        assert_eq!(put["client"], owner, "{put}");
        assert_eq!(put["applied"], true, "{put}");
        let version = put["entries"][0]["version"].as_u64().expect("version");
        assert_eq!(put["revision"], json!(version), "{put}");
        assert_eq!(
            put["entries"],
            json!([{ "key": "step", "value": "5", "version": version }])
        );

        // The service reads it; the reply names the client, not the service.
        let (status, got) = api
            .post(
                "/v1/store",
                &json!({ "party": service_hb, "op": "get", "key": "step" }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{got}");
        assert_eq!(got, put, "the service reads the client's write");

        // The reserved entry nsm_mesh_data: where the claim's parties
        // listen, as the JSON text of an entry at version 0; a write of a
        // reserved key is the caller's fault.
        let (status, mesh) = api
            .post(
                "/v1/store",
                &json!({ "party": service_hb, "op": "get", "key": "nsm_mesh_data" }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{mesh}");
        assert_eq!(mesh["client"], owner, "{mesh}");
        assert_eq!(mesh["entries"][0]["version"], 0, "{mesh}");
        let text = mesh["entries"][0]["value"].as_str().expect("JSON text");
        let data: Value = serde_json::from_str(text).expect("mesh data");
        assert_eq!(data["nsm_key"], "7", "{data}");
        assert_eq!(data["nsm_mesh_client"], client_hb, "{data}");
        assert_eq!(data["nsm_mesh_service"], service_hb, "{data}");
        assert_eq!(data["nsm_service_port"], 9100, "{data}");
        assert_eq!(data["nsm_service_address"], "127.0.0.1", "{data}");
        let (status, body) = api
            .post(
                "/v1/store",
                &json!({ "party": client_hb, "op": "put", "key": "nsm_mesh_data", "value": "x" }),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(
            body["error"].as_str().unwrap_or("").contains("reserved"),
            "{body}"
        );

        // The service writes a second entry; either party lists both.
        let (status, second) = api
            .post(
                "/v1/store",
                &json!({ "party": service_hb, "op": "put", "key": "input/path", "value": "/scratch/in 1.h5" }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{second}");
        assert_eq!(second["client"], owner, "{second}");
        for party in [&client_hb, &service_hb] {
            let (status, listed) = api
                .post("/v1/store", &json!({ "party": party, "op": "list" }))
                .await;
            assert_eq!(status, StatusCode::OK, "{listed}");
            assert_eq!(listed["client"], owner, "{party}: {listed}");
            assert_eq!(listed["revision"], second["revision"], "{party}: {listed}");
            // The stored keys, with the broker's own nsm_ keys among them
            // at version 0.
            let keys: Vec<&str> = listed["entries"]
                .as_array()
                .expect("entries")
                .iter()
                .filter(|e| e["version"] != 0)
                .filter_map(|e| e["key"].as_str())
                .collect();
            assert_eq!(keys, ["input/path", "step"], "{party}: {listed}");
            assert!(
                listed["entries"]
                    .as_array()
                    .expect("entries")
                    .iter()
                    .any(|e| e["key"] == "nsm_mesh_data" && e["version"] == 0),
                "{party}: {listed}"
            );
            assert_eq!(listed["entries"][0]["value"], json!("/scratch/in 1.h5"));
        }

        // A delete answers with the removed entry, then with none; an unset
        // key is still 200, with no entries.
        let delete = json!({ "party": client_hb, "op": "delete", "key": "step" });
        let (status, removed) = api.post("/v1/store", &delete).await;
        assert_eq!(status, StatusCode::OK, "{removed}");
        assert_eq!(removed["entries"], put["entries"], "{removed}");
        let (status, again) = api.post("/v1/store", &delete).await;
        assert_eq!(status, StatusCode::OK, "{again}");
        assert_eq!(again["entries"], json!([]), "{again}");
        assert_eq!(again["revision"], removed["revision"], "no write, no version");
        let (status, unset) = api
            .post(
                "/v1/store",
                &json!({ "party": service_hb, "op": "get", "key": "step" }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{unset}");
        assert_eq!(unset["entries"], json!([]), "{unset}");
        assert_eq!(unset["client"], owner, "{unset}");

        // A service nobody holds reads an empty store and may not write:
        // a get answers with no client and no entry, a list with no client
        // and the broker's own entries alone, put and delete are refused.
        let lonely = cluster.publish(8, 9101).await;
        let lonely_hb = lonely.bound().to_string();
        let (status, empty) = api
            .post(
                "/v1/store",
                &json!({ "party": lonely_hb, "op": "get", "key": "step" }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{empty}");
        assert_eq!(
            empty,
            json!({ "client": null, "revision": 0, "applied": true, "entries": [] })
        );
        let (status, listed) = api
            .post("/v1/store", &json!({ "party": lonely_hb, "op": "list" }))
            .await;
        assert_eq!(status, StatusCode::OK, "{listed}");
        assert_eq!((&listed["client"], &listed["revision"]), (&Value::Null, &json!(0)));
        let entries = listed["entries"].as_array().expect("entries");
        assert!(
            !entries.is_empty()
                && entries.iter().all(|e| {
                    e["version"] == 0 && e["key"].as_str().is_some_and(|k| k.starts_with("nsm_"))
                }),
            "{listed}"
        );
        assert!(
            entries.iter().all(|e| e["key"] != "nsm_mesh_client"),
            "no client yet: {listed}"
        );
        for write in [
            json!({ "party": lonely_hb, "op": "put", "key": "step", "value": "5" }),
            json!({ "party": lonely_hb, "op": "delete", "key": "step" }),
        ] {
            let (status, body) = api.post("/v1/store", &write).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{write}: {body}");
            assert!(
                body["error"].as_str().unwrap_or("").contains("not claimed"),
                "{write}: {body}"
            );
        }

        // Malformed bodies are the caller's fault.
        for bad in [
            json!({ "party": client_hb, "op": "get", "key": "two words" }),
            json!({ "party": client_hb, "op": "get", "key": "-x" }),
            json!({ "party": client_hb, "op": "get" }),
            json!({ "party": client_hb, "op": "frobnicate", "key": "step" }),
            json!({ "party": client_hb, "op": "put", "key": "step" }),
            json!({ "party": client_hb, "key": "step" }),
            json!({ "op": "list" }),
        ] {
            let (status, body) = api.post("/v1/store", &bad).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
            assert!(body["error"].is_string(), "{bad}: {body}");
        }

        // An unreachable party is upstream trouble.
        let dead = Addr::new(Transport::Http, "127.0.0.1", unused_port());
        let (status, body) = api
            .post(
                "/v1/store",
                &json!({ "party": dead.to_string(), "op": "list" }),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
        assert!(body["error"].is_string(), "{body}");

        api.stop().await;
        cluster.stop().await;
    })
    .await;
}

#[tokio::test]
async fn store_by_key_through_the_api() {
    with_deadline(async {
        let cluster = Cluster::start(Transport::Http).await;
        let api = Api::start(cluster.net().clone(), None).await;
        let broker = cluster.broker_addr().to_string();
        let key: Key = "job-17/step.2".parse().unwrap();
        let service = cluster.publish(key.clone(), 9100).await;
        let client = cluster.claim(key.clone()).await;
        let client_hb = client.bound().to_string();
        let owner = json!(client.id());

        // By key: the claim's store, the same reply as through a party.
        let (status, put) = api
            .post(
                "/v1/store",
                &json!({ "broker": broker, "rendezvous": "job-17/step.2", "op": "put", "key": "step", "value": "5" }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{put}");
        assert_eq!(put["client"], owner, "{put}");
        let (status, got) = api
            .post(
                "/v1/store",
                &json!({ "party": client_hb, "op": "get", "key": "step" }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{got}");
        assert_eq!(got, put);
        // party_id names a party of the key; the mesh data resolves alike.
        let (status, got) = api
            .post(
                "/v1/store",
                &json!({ "broker": broker, "rendezvous": "job-17/step.2", "party_id": service.id(), "op": "get", "key": "step" }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{got}");
        assert_eq!(got, put);
        let (status, mesh) = api
            .post(
                "/v1/store",
                &json!({ "broker": broker, "rendezvous": "job-17/step.2", "op": "get", "key": "nsm_mesh_data" }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{mesh}");
        let text = mesh["entries"][0]["value"].as_str().expect("JSON text");
        let data: Value = serde_json::from_str(text).expect("mesh data");
        assert_eq!(data["nsm_mesh_client"], client_hb, "{data}");
        assert_eq!(data["nsm_mesh_service"], service.bound().to_string(), "{data}");

        // The broker's refusals are 400 with the reason.
        for (body, needle) in [
            (
                json!({ "broker": broker, "rendezvous": "job-18", "op": "list" }),
                "no party under key job-18",
            ),
            (
                json!({ "broker": broker, "rendezvous": "job-17/step.2", "party_id": 99, "op": "list" }),
                "no party 99 under key job-17/step.2",
            ),
        ] {
            let (status, b) = api.post("/v1/store", &body).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {b}");
            assert!(
                b["error"].as_str().unwrap_or("").contains(needle),
                "{body}: {b}"
            );
        }
        // A body names one form or the other, whole.
        for bad in [
            json!({ "party": client_hb, "broker": broker, "rendezvous": "job-17/step.2", "op": "list" }),
            json!({ "party": client_hb, "rendezvous": "job-17/step.2", "op": "list" }),
            json!({ "party": client_hb, "party_id": 1, "op": "list" }),
            json!({ "broker": broker, "op": "list" }),
            json!({ "rendezvous": "job-17/step.2", "op": "list" }),
            json!({ "broker": broker, "rendezvous": "a b", "op": "list" }),
            json!({ "broker": broker, "rendezvous": "job-17/step.2", "party_id": -1, "op": "list" }),
            json!({ "op": "list" }),
        ] {
            let (status, b) = api.post("/v1/store", &bad).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {b}");
            assert!(b["error"].is_string(), "{bad}: {b}");
        }
        // A broker that cannot be reached is upstream trouble.
        let dead = Addr::new(Transport::Http, "127.0.0.1", unused_port()).to_string();
        let (status, b) = api
            .post(
                "/v1/store",
                &json!({ "broker": dead, "rendezvous": "job-17/step.2", "op": "list" }),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_GATEWAY, "{b}");

        api.stop().await;
        cluster.stop().await;
    })
    .await;
}

#[tokio::test]
async fn conditional_store_writes_through_the_api() {
    with_deadline(async {
        let cluster = Cluster::start(Transport::Http).await;
        let api = Api::start(cluster.net().clone(), None).await;
        let service = cluster.publish(7, 9100).await;
        let client = cluster.claim(7).await;
        let (client_hb, service_hb) = (client.bound().to_string(), service.bound().to_string());
        let owner = json!(client.id());

        // Create-only: the first put with if_version 0 is an ordinary 200.
        let create = |party: &str, value: &str| {
            json!({ "party": party, "op": "put", "key": "task", "value": value, "if_version": 0 })
        };
        let (status, first) = api.post("/v1/store", &create(&client_hb, "a")).await;
        assert_eq!(status, StatusCode::OK, "{first}");
        assert_eq!(first["applied"], true, "{first}");
        assert!(first.get("error").is_none(), "{first}");
        let v1 = first["entries"][0]["version"].as_u64().expect("version");

        // The second is a 409 carrying the reply, applied false and the
        // current entry, with an error saying where the key is.
        let (status, lost) = api.post("/v1/store", &create(&service_hb, "b")).await;
        assert_eq!(status, StatusCode::CONFLICT, "{lost}");
        assert_eq!(
            lost,
            json!({
                "error": format!("store key task is at version {v1}"),
                "client": owner,
                "revision": v1,
                "applied": false,
                "entries": [{ "key": "task", "value": "a", "version": v1 }],
            })
        );

        // A put at the current version is a 200 with the new entry.
        let (status, updated) = api
            .post(
                "/v1/store",
                &json!({ "party": service_hb, "op": "put", "key": "task", "value": "c", "if_version": v1 }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{updated}");
        assert_eq!(updated["applied"], true, "{updated}");
        let v2 = updated["entries"][0]["version"].as_u64().expect("version");
        assert!(v2 > v1, "{updated}");

        // A stale delete misses; a current one removes the key; after that a
        // version other than 0 misses with "not set" and no entries.
        let delete = |if_version: u64| {
            json!({ "party": client_hb, "op": "delete", "key": "task", "if_version": if_version })
        };
        let (status, stale) = api.post("/v1/store", &delete(v1)).await;
        assert_eq!(status, StatusCode::CONFLICT, "{stale}");
        assert_eq!(
            stale["error"],
            json!(format!("store key task is at version {v2}"))
        );
        assert_eq!(stale["entries"][0]["value"], "c", "{stale}");
        let (status, removed) = api.post("/v1/store", &delete(v2)).await;
        assert_eq!(status, StatusCode::OK, "{removed}");
        assert_eq!(removed["entries"][0]["version"], json!(v2), "{removed}");
        let (status, unset) = api.post("/v1/store", &delete(v2)).await;
        assert_eq!(status, StatusCode::CONFLICT, "{unset}");
        assert_eq!(unset["error"], "store key task is not set", "{unset}");
        assert_eq!(unset["applied"], false, "{unset}");
        assert_eq!(unset["entries"], json!([]), "{unset}");
        let (status, nothing) = api.post("/v1/store", &delete(0)).await;
        assert_eq!(status, StatusCode::OK, "{nothing}");
        assert_eq!(nothing["entries"], json!([]), "{nothing}");

        // A null condition is no condition, and a malformed one is the
        // caller's fault.
        let (status, plain) = api
            .post(
                "/v1/store",
                &json!({ "party": client_hb, "op": "put", "key": "task", "value": "d", "if_version": null }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{plain}");
        for bad in [json!(-1), json!("1"), json!(1.5)] {
            let body = json!({ "party": client_hb, "op": "put", "key": "task", "value": "e", "if_version": bad });
            let (status, reply) = api.post("/v1/store", &body).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {reply}");
        }
        // An unclaimed service is refused whatever the condition.
        let lonely = cluster.publish(8, 9101).await;
        let (status, refused) = api
            .post("/v1/store", &create(&lonely.bound().to_string(), "x"))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
        assert!(
            refused["error"]
                .as_str()
                .unwrap_or("")
                .contains("not claimed"),
            "{refused}"
        );

        api.stop().await;
        cluster.stop().await;
    })
    .await;
}

#[tokio::test]
async fn bearer_token_guards_every_route() {
    with_deadline(async {
        let api = Api::start(NetOpts::default(), Some("s3cret")).await;
        for (method, path) in [
            (Method::GET, "/healthz"),
            (Method::GET, "/v1/interfaces"),
            (Method::GET, "/v1/jobs"),
            (Method::POST, "/v1/send"),
            (Method::POST, "/v1/store"),
            (Method::DELETE, "/v1/jobs/1"),
        ] {
            let (status, body) = api.send(api.bare(method.clone(), path)).await;
            assert_eq!(
                status,
                StatusCode::UNAUTHORIZED,
                "{method} {path} without a token"
            );
            assert!(
                body["error"].as_str().unwrap_or("").contains("bearer"),
                "{body}"
            );
            let wrong = api.bare(method.clone(), path).bearer_auth("s3cre");
            assert_eq!(api.send(wrong).await.0, StatusCode::UNAUTHORIZED, "{path}");
            let longer = api.bare(method.clone(), path).bearer_auth("s3cretx");
            assert_eq!(api.send(longer).await.0, StatusCode::UNAUTHORIZED, "{path}");
            let basic = api
                .bare(method, path)
                .header("authorization", "Basic czNjcmV0");
            assert_eq!(api.send(basic).await.0, StatusCode::UNAUTHORIZED, "{path}");
        }
        assert_eq!(api.get("/healthz").await.0, StatusCode::OK);
        assert_eq!(api.get("/v1/jobs").await.1, json!([]));
        api.stop().await;

        // Off loopback the token is mandatory, and checked before binding.
        for token in [None, Some("")] {
            let err = serve(
                ServeOpts {
                    bind: "0.0.0.0:0".parse().unwrap(),
                    token: token.map(str::to_owned),
                    net: NetOpts::default(),
                },
                CancellationToken::new(),
            )
            .await
            .expect_err("refused");
            assert!(matches!(err, nsm::Error::Config(_)), "{err:?}");
            assert!(err.to_string().contains("token"), "{err}");
        }
    })
    .await;
}

//! Integration tests for the REST control plane (`nsm serve`): every route,
//! the job lifecycle, error mapping and the bearer token, with a broker
//! running in the same process.

mod common;

use std::time::Duration;

use nsm::net::{Addr, Transport};
use nsm::ops::NetOpts;
use nsm::rest::{serve, ControlPlane, ServeOpts};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};
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
fn party_body(broker: &Addr, key: u64, extra: Value) -> Value {
    let mut body = json!({
        "broker": broker.to_string(),
        "key": key,
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
        assert_eq!(publish["key"], json!(7));
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
        assert_eq!(collected["text"], Value::Null);
        assert_eq!(collected["service"]["service_port"], json!(9100));

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
async fn bearer_token_guards_every_route() {
    with_deadline(async {
        let api = Api::start(NetOpts::default(), Some("s3cret")).await;
        for (method, path) in [
            (Method::GET, "/healthz"),
            (Method::GET, "/v1/interfaces"),
            (Method::GET, "/v1/jobs"),
            (Method::POST, "/v1/send"),
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

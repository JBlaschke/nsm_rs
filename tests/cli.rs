//! Tests of the `nsm` binary itself: argument parsing and exit codes, output
//! discipline (results on stdout, everything else on stderr), and complete
//! broker/service/client sessions driven through the command line.

use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_nsm");
/// Loopback selection that works on Linux (`lo`) and macOS (`lo0`) alike.
const IFACE: &[&str] = &["-i", "127.", "--ip-version", "4"];
/// Fast timings so failure paths complete in seconds.
const FAST: &[&str] = &[
    "--heartbeat-interval",
    "0.1",
    "--heartbeat-timeout",
    "0.5",
    "--fail-threshold",
    "3",
    "--ping-staleness",
    "1",
    "--broker-watchdog",
    "1.5",
    "--request-timeout",
    "2",
    "--connect-timeout",
    "1",
];
const WAIT: Duration = Duration::from_secs(20);

fn nsm() -> Command {
    let mut c = Command::new(BIN);
    c.env("NSM_LOG_LEVEL", "warn").env("NSM_LOG_STYLE", "never");
    for var in ["NSM_TOKEN", "CERT_PATH", "KEY_PATH", "ROOT_PATH"] {
        c.env_remove(var);
    }
    c.stdin(Stdio::null());
    c
}

struct Output {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run<I, S>(args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let out = nsm().args(args).output().expect("nsm runs");
    Output {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn argv<'a>(parts: &[&[&'a str]]) -> Vec<&'a str> {
    parts.concat()
}

fn unused_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn pump<R: Read + Send + 'static>(reader: R) -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

/// A long-running `nsm` process whose stdout and stderr are read line by
/// line on threads. Killed on drop.
struct Proc {
    name: &'static str,
    child: Child,
    stdout: Receiver<String>,
    stderr: Receiver<String>,
    seen_stderr: Vec<String>,
}

impl Proc {
    fn spawn(name: &'static str, args: &[&str]) -> Proc {
        Self::spawn_with_env(name, args, &[])
    }

    fn spawn_with_env(name: &'static str, args: &[&str], env: &[(&str, &str)]) -> Proc {
        let mut child = nsm()
            .args(args)
            .envs(env.iter().copied())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn nsm");
        let stdout = pump(child.stdout.take().expect("stdout"));
        let stderr = pump(child.stderr.take().expect("stderr"));
        Proc {
            name,
            child,
            stdout,
            stderr,
            seen_stderr: Vec::new(),
        }
    }

    fn drain(&mut self) {
        while let Ok(line) = self.stderr.try_recv() {
            self.seen_stderr.push(line);
        }
    }

    fn exited_early(&mut self, waiting_for: &str) {
        if let Ok(Some(status)) = self.child.try_wait() {
            self.drain();
            panic!(
                "{}: exited with {status} before {waiting_for}; stderr:\n{}",
                self.name,
                self.seen_stderr.join("\n")
            );
        }
    }

    /// The next stderr line containing `needle`.
    fn stderr_line_containing(&mut self, needle: &str) -> String {
        let deadline = Instant::now() + WAIT;
        loop {
            match self.stderr.recv_timeout(Duration::from_millis(100)) {
                Ok(line) => {
                    self.seen_stderr.push(line.clone());
                    if line.contains(needle) {
                        return line;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    self.exited_early(&format!("printing {needle:?}"))
                }
                Err(RecvTimeoutError::Disconnected) => {
                    self.exited_early(&format!("printing {needle:?}"))
                }
            }
            assert!(
                Instant::now() < deadline,
                "{}: no stderr line containing {needle:?} within {WAIT:?}; seen:\n{}",
                self.name,
                self.seen_stderr.join("\n")
            );
        }
    }

    /// The next stdout line (a result).
    fn stdout_line(&mut self) -> String {
        let deadline = Instant::now() + WAIT;
        loop {
            match self.stdout.recv_timeout(Duration::from_millis(100)) {
                Ok(line) => return line,
                Err(_) => self.exited_early("printing a result"),
            }
            assert!(
                Instant::now() < deadline,
                "{}: no stdout line within {WAIT:?}",
                self.name
            );
        }
    }

    fn wait_exit(&mut self) -> ExitStatus {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(status) = self.child.try_wait().expect("try_wait") {
                self.drain();
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "{}: still running after {WAIT:?}",
                self.name
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(unix)]
    fn terminate(&mut self) -> ExitStatus {
        let status = Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status()
            .expect("kill runs");
        assert!(status.success(), "kill -TERM failed");
        self.wait_exit()
    }

    #[cfg(not(unix))]
    fn terminate(&mut self) -> ExitStatus {
        let _ = self.child.kill();
        self.wait_exit()
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        self.kill();
    }
}

/// `... (heartbeats on X)` → `X`.
fn heartbeat_addr(line: &str) -> String {
    let marker = "(heartbeats on ";
    let start = line.find(marker).expect("heartbeat marker") + marker.len();
    let end = line[start..].find(')').expect("closing paren") + start;
    line[start..end].to_owned()
}

#[test]
fn version_and_help_for_every_command() {
    let out = run(["--version"]);
    assert_eq!(out.code, 0);
    assert!(out.stdout.starts_with("nsm 0.1.0"), "{}", out.stdout);
    assert!(out.stderr.is_empty());

    let out = run(["--help"]);
    assert_eq!(out.code, 0);
    for cmd in [
        "list-interfaces",
        "list-ips",
        "listen",
        "publish",
        "claim",
        "collect",
        "send",
        "serve",
    ] {
        assert!(out.stdout.contains(cmd), "top-level help lacks {cmd}");
        let sub = run([cmd, "--help"]);
        assert_eq!(sub.code, 0, "{cmd} --help");
        assert!(
            sub.stdout.contains("Usage:"),
            "{cmd} --help: {}",
            sub.stdout
        );
    }

    let listen = run(["listen", "--help"]).stdout;
    for flag in [
        "--bind-port",
        "--transport",
        "--tls-cert",
        "--tls-key",
        "--root-ca",
        "--system-roots",
        "--heartbeat-interval",
        "--heartbeat-timeout",
        "--fail-threshold",
        "--ping-staleness",
        "--broker-watchdog",
        "--request-timeout",
        "--connect-timeout",
        "--max-frame-bytes",
        "--max-connections",
        "--max-registrations",
        "--require-matching-host",
        "--max-registrations-per-host",
        "--ip-version",
        "--name",
        "--ip-start",
        "--log-level",
    ] {
        assert!(listen.contains(flag), "listen --help lacks {flag}");
    }
    assert!(run(["serve", "--help"]).stdout.contains("--token"));
    assert!(
        !run(["collect", "--help"]).stdout.contains("--key"),
        "collect's legacy --key stays hidden"
    );
}

#[test]
fn usage_errors_exit_2_and_explain() {
    let cases: &[(&[&str], &str)] = &[
        (&[], "Usage"),
        (&["frobnicate"], "unrecognized subcommand"),
        (&["claim", "127.0.0.1:1", "--bind-port", "0"], "--key"),
        (&["collect", "ftp://host:1"], "unsupported scheme"),
        (&["collect", "127.0.0.1"], "port is required"),
        (&["collect", "127.0.0.1:99999"], "port"),
        (&["collect", "http://host:1/path"], "path"),
        (&["listen", "--bind-port", "70000"], "70000"),
        (
            &["listen", "--bind-port", "0", "--fail-threshold", "0"],
            "not in",
        ),
        (
            &["listen", "--bind-port", "0", "--heartbeat-interval", "abc"],
            "abc",
        ),
        (
            &["listen", "--bind-port", "0", "--max-frame-bytes", "10"],
            "not in",
        ),
        (
            &["listen", "--bind-port", "0", "--transport", "smtp"],
            "smtp",
        ),
        (&["list-ips", "--ip-version", "5"], "5"),
        (&["serve", "--bind", "not-an-address"], "not-an-address"),
    ];
    for (args, needle) in cases {
        let out = run(*args);
        assert_eq!(
            out.code, 2,
            "{args:?}: stdout={:?} stderr={:?}",
            out.stdout, out.stderr
        );
        assert!(
            out.stdout.is_empty(),
            "{args:?} wrote to stdout: {}",
            out.stdout
        );
        assert!(out.stderr.contains(needle), "{args:?}: {}", out.stderr);
    }
}

#[test]
fn listing_commands_print_results_on_stdout_only() {
    let plain = run(["list-interfaces"]);
    assert_eq!(plain.code, 0, "{}", plain.stderr);
    assert!(plain.stderr.is_empty(), "{}", plain.stderr);
    let names: Vec<&str> = plain.stdout.lines().collect();
    assert!(!names.is_empty());
    let alias = run(["list_interfaces"]);
    assert_eq!((alias.code, alias.stdout), (0, plain.stdout.clone()));
    let verbose = run(["list-interfaces", "-v"]);
    assert!(
        verbose.stdout.starts_with("Interfaces:\n"),
        "{}",
        verbose.stdout
    );
    assert!(verbose.stdout.contains(&format!(" - {}", names[0])));

    let ips = run(argv(&[&["list-ips"], IFACE]));
    assert_eq!(ips.code, 0, "{}", ips.stderr);
    assert_eq!(ips.stdout.trim(), "127.0.0.1");
    let verbose = run(argv(&[&["list_ips", "-v"], IFACE]));
    assert!(
        verbose.stdout.starts_with("Addresses:\n"),
        "{}",
        verbose.stdout
    );
    assert!(
        verbose.stdout.contains(" - 127.0.0.1 ("),
        "{}",
        verbose.stdout
    );
    let none = run(["list-ips", "-n", "no-such-interface0"]);
    assert_eq!((none.code, none.stdout.as_str()), (0, ""));
}

#[test]
fn runtime_failures_exit_1_with_a_prefixed_message() {
    let dead = format!("127.0.0.1:{}", unused_port());
    for args in [
        argv(&[&["collect", &dead], FAST]),
        argv(&[&["send", &dead, "--msg", "x"], FAST]),
        argv(&[&["collect", &format!("http://{dead}")], FAST]),
    ] {
        let out = run(&args);
        assert_eq!(out.code, 1, "{args:?}: {}", out.stderr);
        assert!(out.stdout.is_empty(), "{args:?}: {}", out.stdout);
        assert!(out.stderr.starts_with("nsm: "), "{args:?}: {}", out.stderr);
    }

    let out = run(["listen", "--bind-port", "0", "-n", "no-such-interface0"]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.starts_with("nsm: "), "{}", out.stderr);

    let out = run(["serve", "--bind", "0.0.0.0:0"]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("token"), "{}", out.stderr);

    // TLS needs an identity to serve and a trust root to dial.
    let out = run(argv(&[
        &["listen", "--bind-port", "0", "--transport", "tls"],
        IFACE,
    ]));
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("--tls-cert"), "{}", out.stderr);
    for scheme in ["tls", "https"] {
        let out = run(argv(&[&["collect", &format!("{scheme}://{dead}")], FAST]));
        assert_eq!(out.code, 1, "{scheme}: {}", out.stderr);
        assert!(
            out.stderr.contains("--root-ca"),
            "{scheme}: the missing trust root is reported before dialling: {}",
            out.stderr
        );
    }
}

/// Broker, service and client as separate processes: pairing, send/collect,
/// refusals, and what happens when the broker goes away.
fn full_session(transport: &str) {
    let mut broker = Proc::spawn(
        "listen",
        &argv(&[
            &["listen", "--bind-port", "0", "--transport", transport],
            IFACE,
            FAST,
        ]),
    );
    let line = broker.stderr_line_containing("broker listening on ");
    let broker_addr = line.rsplit(' ').next().unwrap().to_owned();
    let prefix = if transport == "tcp" {
        "127.0.0.1:"
    } else {
        "http://127.0.0.1:"
    };
    assert!(broker_addr.starts_with(prefix), "{broker_addr}");

    let mut service = Proc::spawn(
        "publish",
        &argv(&[
            &[
                "publish",
                &broker_addr,
                "--bind-port",
                "0",
                "--service-port",
                "9000",
                "--key",
                "1234",
            ],
            IFACE,
            FAST,
        ]),
    );
    let service_hb = heartbeat_addr(&service.stderr_line_containing("service registered as "));
    assert!(service_hb.starts_with(prefix), "{service_hb}");

    let mut client = Proc::spawn(
        "claim",
        &argv(&[
            &["claim", &broker_addr, "--bind-port", "0", "--key", "1234"],
            IFACE,
            FAST,
        ]),
    );
    assert_eq!(client.stdout_line(), "127.0.0.1:9000");
    let client_hb = heartbeat_addr(&client.stderr_line_containing("client registered as "));
    assert!(client_hb.starts_with(prefix), "{client_hb}");

    let out = run(argv(&[&["send", &client_hb, "--msg", "hello job"], FAST]));
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stdout.is_empty(), "{}", out.stdout);
    assert!(out.stderr.contains("nsm: delivered"), "{}", out.stderr);

    // The text rides on the service's next heartbeat.
    let deadline = Instant::now() + WAIT;
    loop {
        let out = run(argv(&[&["collect", &service_hb], FAST]));
        assert_eq!(out.code, 0, "{}", out.stderr);
        if out.stdout.trim() == "hello job" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "service never received the text; last output {:?} / {:?}",
            out.stdout,
            out.stderr
        );
        thread::sleep(Duration::from_millis(50));
    }
    let out = run(argv(&[&["collect", &client_hb], FAST]));
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.stdout.trim(), "127.0.0.1:9000");

    // Refusals: an unknown key, and a key whose only service is taken.
    for key in ["999", "1234"] {
        let out = run(argv(&[
            &["claim", &broker_addr, "--bind-port", "0", "--key", key],
            IFACE,
            FAST,
        ]));
        assert_eq!(out.code, 1, "key {key}: {}", out.stderr);
        assert!(out.stdout.is_empty(), "key {key}: {}", out.stdout);
        assert!(
            out.stderr.starts_with("nsm: ") && out.stderr.contains(key),
            "key {key}: {}",
            out.stderr
        );
    }

    // SIGTERM stops the broker cleanly; the parties notice within the
    // watchdog and exit non-zero with a message.
    let status = broker.terminate();
    assert_eq!(
        status.code(),
        Some(0),
        "broker exit after SIGTERM: {status}"
    );
    for party in [&mut service, &mut client] {
        let status = party.wait_exit();
        assert_eq!(status.code(), Some(1), "{}: {status}", party.name);
        assert!(
            party.seen_stderr.iter().any(|l| l.starts_with("nsm: ")),
            "{}: {:?}",
            party.name,
            party.seen_stderr
        );
    }
}

#[test]
fn full_session_over_tcp() {
    full_session("tcp");
}

#[test]
fn full_session_over_http() {
    full_session("http");
}

#[tokio::test]
async fn serve_runs_the_control_plane_with_a_token() {
    nsm::tls::install_default_provider();
    let http = reqwest::Client::new();
    for (args, env, token) in [
        (
            &["serve", "--bind", "127.0.0.1:0", "--token", "s3cret"][..],
            &[][..],
            "s3cret",
        ),
        (
            &["serve", "--bind", "127.0.0.1:0"][..],
            &[("NSM_TOKEN", "fromenv")][..],
            "fromenv",
        ),
    ] {
        let mut control = Proc::spawn_with_env("serve", args, env);
        let line = control.stderr_line_containing("control plane listening on ");
        let url = line.rsplit(' ').next().unwrap().to_owned();
        assert!(url.starts_with("http://127.0.0.1:"), "{url}");
        let anon = http.get(format!("{url}/healthz")).send().await.unwrap();
        assert_eq!(anon.status(), 401, "{args:?}");
        let ok = http
            .get(format!("{url}/healthz"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap();
        assert_eq!(ok.status(), 200, "{args:?}");
        assert_eq!(ok.json::<serde_json::Value>().await.unwrap()["ok"], true);
        let status = control.terminate();
        assert_eq!(status.code(), Some(0), "serve exit after SIGTERM: {status}");
    }
}

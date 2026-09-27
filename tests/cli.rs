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
        "peer",
        "send",
        "store",
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
    let store = run(["store", "--help"]).stdout;
    for op in ["get", "put", "delete", "list"] {
        assert!(store.contains(op), "store --help lacks {op}");
        let sub = run(["store", op, "--help"]);
        assert_eq!(sub.code, 0, "store {op} --help");
        for text in [
            "Usage:",
            "<PARTY>",
            "--json",
            "--root-ca",
            "--request-timeout",
        ] {
            assert!(
                sub.stdout.contains(text),
                "store {op} --help lacks {text}: {}",
                sub.stdout
            );
        }
        assert!(
            !sub.stdout.contains("--ip-start"),
            "store {op} takes no interface options"
        );
    }
    assert!(run(["store", "put", "--help"]).stdout.contains("--value"));

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
        "--max-store-bytes",
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
        (
            &["listen", "--bind-port", "0", "--max-store-bytes", "100"],
            "not in",
        ),
        (&["list-ips", "--ip-version", "5"], "5"),
        (
            &["store", "get", "127.0.0.1:1", "two words"],
            "a store key is",
        ),
        (
            &["store", "put", "127.0.0.1:1", "a*b", "--value", "1"],
            "a store key is",
        ),
        (&["store", "delete", "127.0.0.1:1", "-x"], "-x"),
        (&["store", "put", "127.0.0.1:1", "step"], "--value"),
        (&["store", "list"], "<PARTY>"),
        (&["store", "frobnicate"], "unrecognized subcommand"),
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

/// A reader that went away is a failure like any other: exit 1 with a
/// message, not the panic (exit 101) `println!` gives on a closed pipe.
#[test]
fn a_closed_stdout_exits_1_with_a_message() {
    let (reader, writer) = std::io::pipe().expect("pipe");
    drop(reader);
    let out = nsm()
        .arg("list-interfaces")
        .stdout(writer)
        .stderr(Stdio::piped())
        .output()
        .expect("nsm runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.starts_with("nsm: "), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
}

#[test]
fn runtime_failures_exit_1_with_a_prefixed_message() {
    let dead = format!("127.0.0.1:{}", unused_port());
    for args in [
        argv(&[&["collect", &dead], FAST]),
        argv(&[&["peer", &dead], FAST]),
        argv(&[&["send", &dead, "--msg", "x"], FAST]),
        argv(&[&["store", "get", &dead, "step"], FAST]),
        argv(&[&["store", "put", &dead, "step", "--value", "5"], FAST]),
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
    // A store budget whose full reply would not fit the frame limit.
    let out = run(argv(&[
        &["listen", "--bind-port", "0", "--max-frame-bytes", "4096"],
        IFACE,
    ]));
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(
        out.stderr
            .starts_with("nsm: configuration error: --max-store-bytes 16384"),
        "{}",
        out.stderr
    );
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
/// the shared store, refusals, and what happens when the broker goes away.
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

    // Nobody holds the service yet: its store reads as empty ("not yet",
    // exit 3, and an empty list) and refuses writes (exit 1), so a service
    // that starts first can poll for its input.
    let out = run(argv(&[&["store", "get", &service_hb, "input"], FAST]));
    assert_eq!((out.code, out.stdout.as_str()), (3, ""), "{}", out.stderr);
    assert_eq!(out.stderr, "nsm: input is not set\n");
    let out = run(argv(&[&["store", "list", &service_hb], FAST]));
    assert_eq!((out.code, out.stdout.as_str()), (0, ""), "{}", out.stderr);
    assert!(out.stderr.is_empty(), "{}", out.stderr);
    // The README's failover recipe waits for exactly this text to go away.
    let out = run(argv(&[&["store", "list", &service_hb, "--json"], FAST]));
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(
        out.stdout,
        "{\"client\":null,\"revision\":0,\"entries\":[]}\n"
    );
    for write in [
        &["put", service_hb.as_str(), "input", "--value", "x"][..],
        &["delete", service_hb.as_str(), "input"][..],
    ] {
        let out = run(argv(&[&["store"], write, FAST]));
        assert_eq!(
            (out.code, out.stdout.as_str()),
            (1, ""),
            "{write:?}: {}",
            out.stderr
        );
        assert!(
            out.stderr.starts_with("nsm: ") && out.stderr.contains("not claimed"),
            "{write:?}: {}",
            out.stderr
        );
    }

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

    // Reached, but nothing delivered yet: exit 3, so a polling script can
    // tell "not yet" from "failed".
    let out = run(argv(&[&["collect", &service_hb], FAST]));
    assert_eq!((out.code, out.stdout.as_str()), (3, ""), "{}", out.stderr);
    assert!(
        out.stderr.contains("nothing to collect yet"),
        "{}",
        out.stderr
    );

    let out = run(argv(&[&["send", &client_hb, "--msg", "hello job"], FAST]));
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stdout.is_empty(), "{}", out.stdout);
    assert!(out.stderr.contains("nsm: delivered"), "{}", out.stderr);

    // The text rides on the service's next heartbeat; until then every
    // poll is exit 3 with nothing on stdout, the way a script would wait.
    let deadline = Instant::now() + WAIT;
    loop {
        let out = run(argv(&[&["collect", &service_hb], FAST]));
        if out.code == 0 {
            assert_eq!(out.stdout.trim(), "hello job", "{}", out.stderr);
            break;
        }
        assert_eq!((out.code, out.stdout.as_str()), (3, ""), "{}", out.stderr);
        assert!(
            Instant::now() < deadline,
            "service never received the text; last output {:?} / {:?}",
            out.stdout,
            out.stderr
        );
        thread::sleep(Duration::from_millis(50));
    }
    // One verb per question: `peer` is the client's service and `collect`
    // a party's text; asking a service for its peer is an error naming it.
    let out = run(argv(&[&["peer", &client_hb], FAST]));
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.stdout.trim(), "127.0.0.1:9000");
    let out = run(argv(&[&["peer", &service_hb], FAST]));
    assert_eq!((out.code, out.stdout.as_str()), (1, ""), "{}", out.stderr);
    assert!(
        out.stderr.starts_with("nsm: ") && out.stderr.contains("is a service"),
        "{}",
        out.stderr
    );
    // Nothing has been sent to the client yet.
    let out = run(argv(&[&["collect", &client_hb], FAST]));
    assert_eq!((out.code, out.stdout.as_str()), (3, ""), "{}", out.stderr);

    // And back: the service answers the client holding it.
    let out = run(argv(&[&["send", &service_hb, "--msg", "ready"], FAST]));
    assert_eq!(out.code, 0, "{}", out.stderr);
    let deadline = Instant::now() + WAIT;
    loop {
        let out = run(argv(&[&["collect", &client_hb], FAST]));
        if out.code == 0 {
            assert_eq!(out.stdout.trim(), "ready", "{}", out.stderr);
            break;
        }
        assert_eq!((out.code, out.stdout.as_str()), (3, ""), "{}", out.stderr);
        assert!(
            Instant::now() < deadline,
            "client never received the text; last output {:?} / {:?}",
            out.stdout,
            out.stderr
        );
        thread::sleep(Duration::from_millis(50));
    }

    store_session(&client_hb, &service_hb);

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

/// `nsm store` between a client and the service it holds: what each
/// subcommand prints on stdout and stderr, and its exit status.
fn store_session(client_hb: &str, service_hb: &str) {
    let store = |args: &[&str]| run(argv(&[&["store"], args, FAST]));

    // A put prints the write's version and nothing else.
    let out = store(&["put", client_hb, "step", "--value", "5"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stderr.is_empty(), "{}", out.stderr);
    let first: u64 = out
        .stdout
        .strip_suffix('\n')
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| panic!("put printed {:?}", out.stdout));
    assert!(first > 0);

    // The service reads it at once: the store is the broker's, not a copy.
    let out = store(&["get", service_hb, "step"]);
    assert_eq!(
        (out.code, out.stdout.as_str()),
        (0, "5\n"),
        "{}",
        out.stderr
    );
    assert!(out.stderr.is_empty(), "{}", out.stderr);

    // Values are verbatim: spaces and a leading hyphen round-trip.
    for (key, value) in [("input/path", "/scratch/run 17/in.h5"), ("offset", "-5")] {
        let out = store(&["put", service_hb, key, "--value", value]);
        assert_eq!(out.code, 0, "{key}: {}", out.stderr);
        let version: u64 = out.stdout.trim().parse().expect("a version");
        assert!(version > first, "{key}: versions only grow");
        let out = store(&["get", client_hb, key]);
        assert_eq!(out.code, 0, "{key}: {}", out.stderr);
        assert_eq!(out.stdout, format!("{value}\n"), "{key}");
    }

    // Either party lists the same keys, sorted, one per line.
    for party in [client_hb, service_hb] {
        let out = store(&["list", party]);
        assert_eq!(out.code, 0, "{}", out.stderr);
        assert_eq!(out.stdout, "input/path\noffset\nstep\n", "{party}");
        assert!(out.stderr.is_empty(), "{}", out.stderr);
    }

    // An unset key is "not yet": exit 3, nothing on stdout.
    let out = store(&["get", service_hb, "missing"]);
    assert_eq!((out.code, out.stdout.as_str()), (3, ""), "{}", out.stderr);
    assert_eq!(out.stderr, "nsm: missing is not set\n");

    // --json is one line, the reply as the control plane returns it; the
    // exit status stays what it is without it.
    let out = store(&["get", client_hb, "step", "--json"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.stdout.lines().count(), 1, "{}", out.stdout);
    let reply: serde_json::Value = serde_json::from_str(&out.stdout).expect("JSON");
    assert!(reply["client"].is_u64(), "{reply}");
    assert!(reply["revision"].as_u64() > Some(first), "{reply}");
    assert_eq!(reply["entries"][0]["key"], "step", "{reply}");
    assert_eq!(reply["entries"][0]["value"], "5", "{reply}");
    assert_eq!(reply["entries"][0]["version"], first, "{reply}");
    let out = store(&["get", client_hb, "missing", "--json"]);
    assert_eq!(out.code, 3, "{}", out.stderr);
    let reply: serde_json::Value = serde_json::from_str(&out.stdout).expect("JSON");
    assert_eq!(reply["entries"], serde_json::json!([]), "{reply}");
    let out = store(&["list", service_hb, "--json"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    let reply: serde_json::Value = serde_json::from_str(&out.stdout).expect("JSON");
    assert_eq!(
        reply["entries"].as_array().map(Vec::len),
        Some(3),
        "{reply}"
    );

    // With --json a put prints the reply in place of the version, and
    // nothing on stderr.
    let out = store(&["put", client_hb, "scratch", "--value", "x", "--json"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stderr.is_empty(), "{}", out.stderr);
    assert_eq!(out.stdout.lines().count(), 1, "{}", out.stdout);
    let written: serde_json::Value = serde_json::from_str(&out.stdout).expect("JSON");
    let version = written["revision"].as_u64().expect("revision");
    assert_eq!(
        written["entries"],
        serde_json::json!([{ "key": "scratch", "value": "x", "version": version }]),
        "{written}"
    );

    // A delete with --json keeps its note on stderr and prints the reply:
    // the removed entry, then no entry; exit 0 both times.
    let out = store(&["delete", service_hb, "scratch", "--json"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.stderr, "nsm: deleted scratch\n");
    assert_eq!(out.stdout.lines().count(), 1, "{}", out.stdout);
    let removed: serde_json::Value = serde_json::from_str(&out.stdout).expect("JSON");
    assert_eq!(removed["entries"], written["entries"], "{removed}");
    let out = store(&["delete", service_hb, "scratch", "--json"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.stderr, "nsm: scratch was not set\n");
    assert_eq!(out.stdout.lines().count(), 1, "{}", out.stdout);
    let again: serde_json::Value = serde_json::from_str(&out.stdout).expect("JSON");
    assert_eq!(again["entries"], serde_json::json!([]), "{again}");

    // A delete prints nothing on stdout and succeeds either way.
    let out = store(&["delete", service_hb, "step"]);
    assert_eq!((out.code, out.stdout.as_str()), (0, ""), "{}", out.stderr);
    assert_eq!(out.stderr, "nsm: deleted step\n");
    let out = store(&["delete", client_hb, "step"]);
    assert_eq!((out.code, out.stdout.as_str()), (0, ""), "{}", out.stderr);
    assert_eq!(out.stderr, "nsm: step was not set\n");
    let out = store(&["get", client_hb, "step"]);
    assert_eq!((out.code, out.stdout.as_str()), (3, ""), "{}", out.stderr);
}

#[test]
fn full_session_over_tcp() {
    full_session("tcp");
}

/// A client prints one stdout line per pairing: when its service dies and
/// the broker re-pairs it, the new address follows the first one, so a
/// script that keeps reading always holds the current service.
#[test]
fn claim_prints_a_line_for_every_pairing() {
    let mut broker = Proc::spawn(
        "listen",
        &argv(&[&["listen", "--bind-port", "0"], IFACE, FAST]),
    );
    let line = broker.stderr_line_containing("broker listening on ");
    let broker_addr = line.rsplit(' ').next().unwrap().to_owned();
    let publish = |name: &'static str, port: &str| {
        let mut service = Proc::spawn(
            name,
            &argv(&[
                &[
                    "publish",
                    &broker_addr,
                    "--bind-port",
                    "0",
                    "--service-port",
                    port,
                    "--key",
                    "77",
                ],
                IFACE,
                FAST,
            ]),
        );
        // Registered in order, so the first one has the lowest id.
        service.stderr_line_containing("service registered as ");
        service
    };
    let mut first = publish("publish-1", "9001");
    let _second = publish("publish-2", "9002");

    let mut client = Proc::spawn(
        "claim",
        &argv(&[
            &["claim", &broker_addr, "--bind-port", "0", "--key", "77"],
            IFACE,
            FAST,
        ]),
    );
    assert_eq!(client.stdout_line(), "127.0.0.1:9001", "lowest id first");

    // The first service dies without a word. The broker notices, re-pairs
    // the client with the spare, and the client reports the new address.
    first.kill();
    assert_eq!(client.stdout_line(), "127.0.0.1:9002");
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

//! The `nsm` binary: installs the crypto provider, initialises logging, parses
//! the command line, runs one operation and prints its result. This is the
//! only place that prints to stdout or decides exit codes: 0 when the
//! operation succeeded, 1 when it failed (`nsm: <error>` on stderr), 2 for a
//! usage error (clap's own), and [`NOTHING_YET`] when the party answered but
//! has nothing to report yet (`collect` before the first text, `peer` before
//! the pairing, `store get` of a key that is not set). Every line of stdout
//! goes through [`print_line`], so a reader that went away (a closed pipe)
//! makes the command fail with exit 1 instead of a panic. The one exception
//! is `claim` after its first line: once the service address is out, a
//! re-pairing line that cannot be written stops the printing, not the party.

use std::io::Write;
use std::process::ExitCode;

use clap::Parser;
use tokio_util::sync::CancellationToken;

use nsm::cli::{Cli, Command, LimitsOpts, StoreCommand, TimingOpts, TlsOpts};
use nsm::net::Transport;
use nsm::ops::{self, NetOpts, StoreOp};
use nsm::{Error, Result};

/// Exit status when the party was reached but has nothing to report yet:
/// `collect` before the first text, `peer` before the pairing, `store get`
/// of a key that is not set. Distinct from
/// a failed operation (1) so a polling script can tell "not yet" from
/// "failed" without parsing stderr.
const NOTHING_YET: u8 = 3;

#[tokio::main]
async fn main() -> ExitCode {
    nsm::tls::install_default_provider();
    let cli = Cli::parse();
    nsm::logging::init(cli.log_level.as_deref());

    let shutdown = CancellationToken::new();
    spawn_signal_handler(shutdown.clone());

    match run(cli.command, shutdown).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("nsm: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Cancel the token on Ctrl-C (and SIGTERM on Unix) so every long-running
/// operation shuts down cleanly instead of being killed mid-heartbeat.
fn spawn_signal_handler(shutdown: CancellationToken) {
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            match signal(SignalKind::terminate()) {
                Ok(mut term) => {
                    tokio::select! {
                        _ = tokio::signal::ctrl_c() => {}
                        _ = term.recv() => {}
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "cannot listen for SIGTERM");
                    let _ = tokio::signal::ctrl_c().await;
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
        tracing::info!("shutdown requested");
        shutdown.cancel();
    });
}

/// Write `line` and a newline to stdout and flush it. A failed write (most
/// often a closed pipe: the reader went away) is an error like any other, so
/// the command exits 1 with a message instead of the panic `println!` gives.
fn print_line(line: impl std::fmt::Display) -> Result<()> {
    let mut out = std::io::stdout().lock();
    out.write_all(format!("{line}\n").as_bytes())?;
    out.flush()?;
    Ok(())
}

fn net_opts(tls: &TlsOpts, timing: &TimingOpts, limits: &LimitsOpts) -> NetOpts {
    NetOpts {
        tls: tls.paths(),
        timing: timing.timing(),
        limits: limits.limits(),
    }
}

/// Whether a party serves TLS on its own listener: when asked with `--tls`
/// (which then needs an identity), or automatically when its broker speaks
/// TLS and an identity is configured.
fn serve_tls(tls: &TlsOpts, broker: Transport) -> Result<bool> {
    if tls.tls && !tls.paths().has_server_identity() {
        return Err(Error::config(
            "--tls needs --tls-cert and --tls-key (or CERT_PATH and KEY_PATH)",
        ));
    }
    Ok(tls.tls || (broker.is_tls() && tls.paths().has_server_identity()))
}

async fn run(command: Command, shutdown: CancellationToken) -> Result<ExitCode> {
    match command {
        Command::ListInterfaces {
            ip_version,
            verbose,
        } => {
            let names = ops::list_interfaces(ip_version)?;
            if verbose {
                print_line("Interfaces:")?;
            }
            for n in names {
                print_line(format_args!("{}{n}", if verbose { " - " } else { "" }))?;
            }
        }
        Command::ListIps { iface, verbose } => {
            let addrs = ops::list_ips(&iface.selector())?;
            if verbose {
                print_line("Addresses:")?;
            }
            for a in addrs {
                if verbose {
                    print_line(format_args!(" - {} ({})", a.ip, a.interface))?;
                } else {
                    print_line(a.ip)?;
                }
            }
        }
        Command::Listen {
            bind_port,
            transport,
            iface,
            tls,
            timing,
            limits,
            policy,
        } => {
            let broker = ops::listen(
                ops::ListenRequest {
                    transport,
                    bind_port,
                    selector: iface.selector(),
                    net: net_opts(&tls, &timing, &limits),
                    policy: policy.policy(),
                },
                shutdown,
            )
            .await?;
            eprintln!("nsm: broker listening on {}", broker.bound());
            broker.run().await?;
        }
        Command::Publish {
            broker,
            bind_port,
            service_port,
            key,
            ping,
            iface,
            tls,
            timing,
        } => {
            let serve_tls = serve_tls(&tls, broker.transport)?;
            let session = ops::publish(ops::PublishRequest {
                broker,
                key,
                bind_port,
                service_port,
                selector: iface.selector(),
                serve_tls,
                ping,
                net: net_opts(&tls, &timing, &LimitsOpts::default()),
            })
            .await?;
            eprintln!(
                "nsm: service registered as {} (heartbeats on {})",
                session.id(),
                session.bound()
            );
            run_session(session, shutdown).await?;
        }
        Command::Claim {
            broker,
            bind_port,
            key,
            ping,
            iface,
            tls,
            timing,
        } => {
            let serve_tls = serve_tls(&tls, broker.transport)?;
            let session = ops::claim(ops::ClaimRequest {
                broker,
                key,
                bind_port,
                selector: iface.selector(),
                serve_tls,
                ping,
                net: net_opts(&tls, &timing, &LimitsOpts::default()),
            })
            .await?;
            let mut pairings = session.pairings();
            let first = pairings.borrow_and_update().clone();
            match first {
                // The service address is the one line of stdout a script
                // needs; if nobody can read it, the claim fails (exit 1).
                Some(service) => print_line(service)?,
                None => eprintln!("nsm: paired, but the broker sent no service handle"),
            }
            eprintln!(
                "nsm: client registered as {} (heartbeats on {})",
                session.id(),
                session.bound()
            );
            // Every re-pairing is one more line, so a script that keeps
            // reading always holds the current service. When the reader
            // goes away after the first line, printing stops with a warning
            // in the log; the party itself keeps running.
            tokio::spawn(async move {
                while pairings.changed().await.is_ok() {
                    let current = pairings.borrow_and_update().clone();
                    if let Some(service) = current
                        && let Err(e) = print_line(service)
                    {
                        tracing::warn!(error = %e, "cannot print the new pairing; no longer printing pairings");
                        break;
                    }
                }
            });
            run_session(session, shutdown).await?;
        }
        Command::Collect {
            party,
            key: _,
            iface: _,
            tls,
            timing,
        } => {
            let net = net_opts(&tls, &timing, &LimitsOpts::default());
            match ops::collect(&party, &net).await?.text() {
                Some(text) => print_line(text)?,
                None => {
                    eprintln!("nsm: nothing to collect yet");
                    return Ok(ExitCode::from(NOTHING_YET));
                }
            }
        }
        Command::Peer { party, tls, timing } => {
            let net = net_opts(&tls, &timing, &LimitsOpts::default());
            match ops::collect(&party, &net).await?.service()? {
                Some(service) => print_line(service)?,
                None => {
                    eprintln!("nsm: not paired yet");
                    return Ok(ExitCode::from(NOTHING_YET));
                }
            }
        }
        Command::Send {
            party,
            msg,
            key: _,
            iface: _,
            tls,
            timing,
        } => {
            ops::send(
                &party,
                msg,
                &net_opts(&tls, &timing, &LimitsOpts::default()),
            )
            .await?;
            eprintln!("nsm: delivered");
        }
        Command::Store { op } => return store(op).await,
        Command::Serve {
            bind,
            token,
            tls,
            timing,
            limits,
        } => {
            let control = nsm::rest::serve(
                nsm::rest::ServeOpts {
                    bind,
                    token,
                    net: net_opts(&tls, &timing, &limits),
                },
                shutdown,
            )
            .await?;
            eprintln!(
                "nsm: control plane listening on http://{}",
                control.local_addr()
            );
            control.run().await?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// `nsm store`: one operation through a party, printed per operation. With
/// `--json` stdout carries the broker's reply as one line instead (the body
/// `POST /v1/store` returns); stderr and the exit status stay the same.
async fn store(command: StoreCommand) -> Result<ExitCode> {
    let (party, op, tls, timing, json) = command.into_parts();
    let net = net_opts(&tls, &timing, &LimitsOpts::default());
    let stored = ops::store(&party, op.clone(), &net).await?;
    let mut lines = Vec::new();
    let mut code = ExitCode::SUCCESS;
    match &op {
        StoreOp::Get { key } => match stored.get(key) {
            Some(entry) => lines.push(entry.value.clone()),
            None => {
                eprintln!("nsm: {key} is not set");
                code = ExitCode::from(NOTHING_YET);
            }
        },
        StoreOp::Put { key, .. } => {
            let entry = stored.get(key).ok_or_else(|| {
                Error::protocol(format!("the reply to a put carries no entry for {key}"))
            })?;
            lines.push(entry.version.to_string());
        }
        StoreOp::Delete { key } => match stored.get(key) {
            Some(_) => eprintln!("nsm: deleted {key}"),
            None => eprintln!("nsm: {key} was not set"),
        },
        StoreOp::List => lines.extend(stored.keys().map(ToString::to_string)),
    }
    if json {
        lines = vec![serde_json::to_string(&stored)?];
    }
    for line in lines {
        print_line(line)?;
    }
    Ok(code)
}

/// Run a party session until Ctrl-C or until the broker is lost.
async fn run_session(session: nsm::party::Session, shutdown: CancellationToken) -> Result<()> {
    let token = session.shutdown_token();
    tokio::spawn(async move {
        shutdown.cancelled().await;
        token.cancel();
    });
    session.run().await
}

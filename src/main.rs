//! The `nsm` binary: installs the crypto provider, initialises logging, parses
//! the command line, runs one operation and prints its result. This is the
//! only place that prints to stdout or decides exit codes.

use std::process::ExitCode;

use clap::Parser;
use tokio_util::sync::CancellationToken;

use nsm::cli::{Cli, Command, LimitsOpts, TimingOpts, TlsOpts};
use nsm::net::Transport;
use nsm::ops::{self, NetOpts};
use nsm::{Error, Result};

#[tokio::main]
async fn main() -> ExitCode {
    nsm::tls::install_default_provider();
    let cli = Cli::parse();
    nsm::logging::init(cli.log_level.as_deref());

    let shutdown = CancellationToken::new();
    spawn_signal_handler(shutdown.clone());

    match run(cli.command, shutdown).await {
        Ok(()) => ExitCode::SUCCESS,
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

async fn run(command: Command, shutdown: CancellationToken) -> Result<()> {
    match command {
        Command::ListInterfaces {
            ip_version,
            verbose,
        } => {
            let names = ops::list_interfaces(ip_version)?;
            if verbose {
                println!("Interfaces:");
            }
            for n in names {
                println!("{}{n}", if verbose { " - " } else { "" });
            }
        }
        Command::ListIps { iface, verbose } => {
            let addrs = ops::list_ips(&iface.selector())?;
            if verbose {
                println!("Addresses:");
            }
            for a in addrs {
                if verbose {
                    println!(" - {} ({})", a.ip, a.interface);
                } else {
                    println!("{}", a.ip);
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
            match session.service() {
                // The service address is the one line of stdout a script needs.
                Some(service) => println!("{service}"),
                None => eprintln!("nsm: paired, but the broker sent no service handle"),
            }
            eprintln!(
                "nsm: client registered as {} (heartbeats on {})",
                session.id(),
                session.bound()
            );
            run_session(session, shutdown).await?;
        }
        Command::Collect {
            party,
            key: _,
            iface: _,
            tls,
            timing,
        } => {
            let collected =
                ops::collect(&party, &net_opts(&tls, &timing, &LimitsOpts::default())).await?;
            match (collected.text, collected.service) {
                (Some(text), _) => println!("{text}"),
                (None, Some(service)) => println!("{service}"),
                (None, None) => eprintln!("nsm: nothing to collect yet"),
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
    Ok(())
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

//! The `nsm` binary: installs the crypto provider, initialises logging, parses
//! the command line and runs one operation. This is the only place that
//! decides exit codes.

use std::process::ExitCode;

use clap::Parser;

#[tokio::main]
async fn main() -> ExitCode {
    nsm::tls::install_default_provider();
    let cli = nsm::cli::Cli::parse();
    nsm::logging::init(cli.log_level.as_deref());

    match nsm::legacy::run(cli.command).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("nsm: {e}");
            ExitCode::FAILURE
        }
    }
}

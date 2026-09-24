//! `tracing` initialisation.
//!
//! Log output goes to stderr so that stdout carries only an operation's
//! result. The filter comes from, in order of precedence, an explicit
//! override (the `--log-level` flag), the `NSM_LOG_LEVEL` environment variable,
//! or `warn`. `NSM_LOG_STYLE` controls colour: `auto` (default), `always` or
//! `never`. Records emitted through the `log` crate by dependencies (and by the
//! legacy modules) are bridged into the same subscriber.

use std::io::IsTerminal;

use tracing_subscriber::EnvFilter;

/// Environment variable holding the default filter directive.
pub const LEVEL_ENV: &str = "NSM_LOG_LEVEL";
/// Environment variable controlling ANSI colour in log output.
pub const STYLE_ENV: &str = "NSM_LOG_STYLE";

/// Install the global subscriber. Safe to call once per process; a second call
/// is ignored so tests that share a process do not fail.
pub fn init(level_override: Option<&str>) {
    let directive = level_override
        .map(str::to_owned)
        .or_else(|| std::env::var(LEVEL_ENV).ok())
        .unwrap_or_else(|| "warn".to_owned());
    let filter = EnvFilter::try_new(&directive).unwrap_or_else(|_| EnvFilter::new("warn"));

    let ansi = match std::env::var(STYLE_ENV).as_deref() {
        Ok("always") => true,
        Ok("never") => false,
        _ => std::io::stderr().is_terminal(),
    };

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(ansi)
        .with_writer(std::io::stderr)
        .with_target(false)
        .try_init();
}

#[cfg(test)]
mod tests {
    #[test]
    fn init_twice_does_not_panic() {
        super::init(Some("debug"));
        super::init(Some("not a valid directive ("));
    }
}

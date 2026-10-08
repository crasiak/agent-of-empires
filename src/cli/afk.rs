use crate::session::{
    afk::{control, Operation},
    Storage,
};
use anyhow::Result;
use clap::{Args, Subcommand};

#[derive(Args)]
pub struct AfkArgs {
    #[command(subcommand)]
    command: AfkCommand,
}

#[derive(Subcommand)]
enum AfkCommand {
    /// Request control-only AFK. Does not authorize autonomous work
    On {
        /// Session ID or title
        session: String,
        /// Explicit expiry, from 1 to 1440 minutes
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..=1440))]
        minutes: u32,
    },
    /// One-file delegation during active work; nonzero nudges require queue-preserving abort
    Delegate {
        session: String,
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..=1440))]
        minutes: u32,
        /// Protocol-3 grant: requests 1..8; settlement_nudges omitted=0, maximum 2
        #[arg(long)]
        grant: std::path::PathBuf,
    },
    /// Export private-ledger audit JSON without payloads or preimages
    Audit { session: String },
    /// Record AFK off even if the integration is unavailable
    Off { session: String },
    /// Probe the current integration; does not enable or renew AFK
    Status { session: String },
}

pub async fn run(profile: &str, args: AfkArgs) -> Result<()> {
    if let AfkCommand::Delegate { session, .. } | AfkCommand::Audit { session } = &args.command {
        let storage = Storage::open_unwatched(profile)?;
        let (mut rows, _) = storage.load_with_groups()?;
        for row in &mut rows {
            row.source_profile = profile.to_owned();
        }
        let instance = super::resolve_session(session, &rows)?.clone();
        let result = tokio::task::spawn_blocking(move || match args.command {
            AfkCommand::Delegate { minutes, grant, .. } => {
                crate::session::afk::delegation::delegate(&instance, minutes, &grant)
            }
            AfkCommand::Audit { .. } => crate::session::afk::delegation::audit(&instance),
            _ => unreachable!(),
        })
        .await??;
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }
    let (identifier, operation) = match args.command {
        AfkCommand::On { session, minutes } => (session, Operation::On { minutes }),
        AfkCommand::Off { session } => (session, Operation::Off),
        AfkCommand::Status { session } => (session, Operation::Status),
        _ => unreachable!(),
    };
    let storage = Storage::open_unwatched(profile)?;
    let (mut rows, _) = storage.load_with_groups()?;
    for row in &mut rows {
        row.source_profile = profile.to_owned();
    }
    let instance = super::resolve_session(&identifier, &rows)?.clone();
    let report = tokio::task::spawn_blocking(move || control(&instance, operation)).await??;
    println!("{}", serde_json::to_string_pretty(&report)?);
    require_acknowledged_activation(operation, &report)
}

fn require_acknowledged_activation(
    operation: Operation,
    report: &crate::session::afk::Report,
) -> Result<()> {
    anyhow::ensure!(
        !matches!(operation, Operation::On { .. })
            || (report.state == crate::session::afk::State::ControlOnly
                && report.requested_on
                && report.observed_at_ms.is_some()),
        "AFK activation was not acknowledged; inspect the structured report"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    #[test]
    fn only_acknowledged_activation_is_success_but_diagnostics_and_durable_off_are_success() {
        use crate::session::afk::{Operation, Report, State};
        for state in [
            State::ControlOnly,
            State::Off,
            State::Expired,
            State::Invalidated,
            State::Unsupported,
            State::Unavailable,
            State::Requested,
        ] {
            let report = Report {
                state,
                requested_on: true,
                revision: 1,
                expires_at_ms: Some(100),
                observed_at_ms: Some(1),
                detail: String::new(),
                delegation: None,
            };
            assert_eq!(
                super::require_acknowledged_activation(Operation::On { minutes: 1 }, &report)
                    .is_ok(),
                state == State::ControlOnly
            );
            assert!(super::require_acknowledged_activation(Operation::Status, &report).is_ok());
            assert!(super::require_acknowledged_activation(Operation::Off, &report).is_ok());
        }
    }
    #[test]
    fn activation_requires_explicit_bounded_duration() {
        for args in [
            vec!["aoe", "session", "afk", "on", "s"],
            vec!["aoe", "session", "afk", "delegate", "s", "--minutes", "1"],
            vec![
                "aoe",
                "session",
                "afk",
                "delegate",
                "s",
                "--grant",
                "grant.json",
            ],
            vec!["aoe", "session", "afk", "on", "s", "--minutes", "0"],
            vec!["aoe", "session", "afk", "on", "s", "--minutes", "1441"],
        ] {
            assert!(crate::cli::Cli::try_parse_from(args).is_err());
        }
        assert!(crate::cli::Cli::try_parse_from([
            "aoe",
            "session",
            "afk",
            "on",
            "s",
            "--minutes",
            "15"
        ])
        .is_ok());
        assert!(crate::cli::Cli::try_parse_from(["aoe", "session", "afk", "off", "s"]).is_ok());
    }
}

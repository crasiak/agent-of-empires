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

impl AfkArgs {
    pub fn is_inspection(&self) -> bool {
        matches!(
            self.command,
            AfkCommand::Overview { .. } | AfkCommand::Batch { .. }
        )
    }
}

#[derive(Subcommand)]
enum AfkCommand {
    /// Inspect a frozen profile-scoped selection; no executable batch controls
    Batch {
        #[command(subcommand)]
        command: BatchCommand,
    },
    /// Read stored AFK summaries across all profiles, without probing or enabling sessions
    Overview {
        /// Hide archived and trashed sessions (shown by default)
        #[arg(long)]
        active_only: bool,
    },
    /// Request control-only AFK. Does not authorize autonomous work
    On {
        /// Session ID or title
        session: String,
        /// Explicit expiry, from 1 to 1440 minutes
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..=1440))]
        minutes: u32,
    },
    /// One-file delegation during active work; nudges and question deferral require qualified native support
    Delegate {
        session: String,
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..=1440))]
        minutes: u32,
        /// Protocol-4 grant: requests 1..8; settlement_nudges 0..2; question_deferrals 0..1, both omitted=0
        #[arg(long)]
        grant: std::path::PathBuf,
    },
    /// Export scoped profile-authority audit and legacy evidence references, without payloads or preimages
    Audit { session: String },
    /// Record AFK off even if the integration is unavailable
    Off { session: String },
    /// Probe the current integration; does not enable or renew AFK
    Status { session: String },
}

#[derive(Subcommand)]
enum BatchCommand {
    /// Export private non-authorizing inspection-v1; apply is unavailable
    Preview {
        /// Private manifest-v1 with one profile, 1..16 exact IDs and explicit protocol-4 grants
        #[arg(long)]
        manifest: std::path::PathBuf,
        /// New private artifact outside app state; never overwrites an existing file
        #[arg(long)]
        output: std::path::PathBuf,
    },
}

pub async fn run(profile: &str, args: AfkArgs) -> Result<()> {
    if let AfkCommand::Batch {
        command: BatchCommand::Preview { manifest, output },
    } = args.command
    {
        let receipt = tokio::task::spawn_blocking(move || {
            crate::session::afk::batch::preview(&manifest, &output)
        })
        .await??;
        println!("{}", serde_json::to_string_pretty(&receipt)?);
        return Ok(());
    }
    if let AfkCommand::Overview { active_only } = &args.command {
        let mut overview =
            tokio::task::spawn_blocking(crate::session::afk::overview::snapshot).await??;
        let complete = overview.complete();
        let mut filtered_sessions = 0;
        if *active_only {
            for profile in &mut overview.profiles {
                let before = profile.sessions.len();
                profile
                    .sessions
                    .retain(|row| (!row.archived && !row.trashed) || !row.errors.is_empty());
                filtered_sessions += before - profile.sessions.len();
            }
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "complete": complete,
                "filtered_sessions": filtered_sessions,
                "overview": overview,
            }))?
        );
        anyhow::ensure!(
            complete,
            "AFK overview is incomplete; inspect profile and session errors"
        );
        return Ok(());
    }
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
    fn batch_requires_explicit_files_and_has_no_apply_command() {
        for args in [
            vec![
                "aoe",
                "session",
                "afk",
                "batch",
                "preview",
                "--manifest",
                "manifest.json",
            ],
            vec![
                "aoe",
                "session",
                "afk",
                "batch",
                "preview",
                "--output",
                "preview.json",
            ],
            vec!["aoe", "session", "afk", "batch", "apply", "preview.json"],
        ] {
            assert!(crate::cli::Cli::try_parse_from(args).is_err());
        }
        assert!(crate::cli::Cli::try_parse_from([
            "aoe",
            "session",
            "afk",
            "batch",
            "preview",
            "--manifest",
            "manifest.json",
            "--output",
            "preview.json"
        ])
        .is_ok());
    }
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

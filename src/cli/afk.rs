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
            AfkCommand::Overview { .. }
                | AfkCommand::Review { .. }
                | AfkCommand::Export { .. }
                | AfkCommand::Batch {
                    command: BatchCommand::Preview { .. }
                }
        )
    }
}

#[derive(Subcommand)]
enum AfkCommand {
    /// Inspect exact stored historical windows, without migrations, probes or authority writes
    Review {
        /// Repeat for up to 16 exact PROFILE/SESSION_ID/WINDOW_UUID identities
        #[arg(long = "select", required = true)]
        selections: Vec<crate::session::afk::review::Selection>,
    },
    /// Publish a private redacted return review, never overwriting a file
    Export {
        #[arg(long = "select", required = true)]
        selections: Vec<crate::session::afk::review::Selection>,
        #[arg(long, value_enum)]
        format: crate::session::afk::review::Format,
        /// New file in an existing owned 0700 directory outside app state
        #[arg(long)]
        output: std::path::PathBuf,
    },
    /// Append an operator note/correction/reviewed claim, never an approval or answer
    Annotate {
        /// Private 0600 JSON request: id, target {selection, decision, original_hash}, kind, body, prior
        #[arg(long)]
        request: std::path::PathBuf,
    },
    /// Preview, explicitly apply, inspect or stop a frozen profile-scoped batch
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
        /// Optional private observed-token/operator-rate USD guard; no guard is implied
        #[arg(long)]
        usage_guard: Option<std::path::PathBuf>,
    },
    /// Export scoped profile-authority audit and legacy evidence references, without payloads or preimages
    Audit { session: String },
    /// Revoke AFK and persist a bulk exclusion even if integration is unavailable
    Off { session: String },
    /// Revoke AFK and exclude the session from future batch selections
    OptOut { session: String },
    /// Clear only the bulk exclusion; grants no authority and requires a fresh preview
    AllowBulk { session: String },
    /// Probe the current integration; does not enable or renew AFK
    Status { session: String },
}

#[derive(Subcommand)]
enum BatchCommand {
    /// Export private preview-v3; never activates on its own
    Preview {
        /// Private manifest-v1 with one profile, 1..16 exact IDs and explicit protocol-4 grants
        #[arg(long)]
        manifest: std::path::PathBuf,
        /// New private artifact outside app state; never overwrites an existing file
        #[arg(long)]
        output: std::path::PathBuf,
    },
    /// Apply the exact reviewed preview; retries only report the original operation
    Apply {
        #[arg(long)]
        preview: std::path::PathBuf,
        #[arg(long)]
        digest: String,
    },
    /// Read an operation in the selected profile, without probing members
    Status { operation: String },
    /// Stop only this operation's original windows, never later replacements
    Off { operation: String },
}

pub async fn run(profile: &str, args: AfkArgs) -> Result<()> {
    if let AfkCommand::Review { selections } | AfkCommand::Export { selections, .. } = &args.command
    {
        let selections = selections.clone();
        let report =
            tokio::task::spawn_blocking(move || crate::session::afk::review::collect(&selections))
                .await??;
        let complete = report.complete;
        match args.command {
            AfkCommand::Review { .. } => {
                let bytes = crate::session::afk::review::render(
                    &report,
                    crate::session::afk::review::Format::Json,
                )?;
                use std::io::Write;
                std::io::stdout().lock().write_all(&bytes)?;
            }
            AfkCommand::Export { format, output, .. } => {
                tokio::task::spawn_blocking(move || {
                    crate::session::afk::review::export(&report, format, &output)
                })
                .await??;
                println!(
                    "{}",
                    serde_json::json!({"exported":true,"complete":complete})
                );
            }
            _ => unreachable!(),
        }
        anyhow::ensure!(
            complete,
            "AFK review is incomplete; inspect per-selection errors"
        );
        return Ok(());
    }
    if let AfkCommand::Annotate { request } = args.command {
        let annotation = tokio::task::spawn_blocking(move || -> Result<_> {
            let request = crate::session::afk::review::read_annotation_request(&request)?;
            crate::session::afk::review::annotations::append(request)
        })
        .await??;
        let bytes = crate::session::afk::review::render_annotation(&annotation)?;
        use std::io::Write;
        std::io::stdout().lock().write_all(&bytes)?;
        return Ok(());
    }
    if matches!(
        &args.command,
        AfkCommand::Batch {
            command: BatchCommand::Apply { .. }
                | BatchCommand::Status { .. }
                | BatchCommand::Off { .. }
        }
    ) {
        let profile = profile.to_owned();
        let value = tokio::task::spawn_blocking(move || match args.command {
            AfkCommand::Batch {
                command: BatchCommand::Apply { preview, digest },
            } => crate::session::afk::batch::apply(&preview, &digest),
            AfkCommand::Batch {
                command: BatchCommand::Status { operation },
            } => crate::session::afk::batch::status(
                &crate::session::resolve_existing_profile(&profile)?,
                &operation,
            ),
            AfkCommand::Batch {
                command: BatchCommand::Off { operation },
            } => crate::session::afk::batch::off(
                &crate::session::resolve_existing_profile(&profile)?,
                &operation,
            ),
            _ => unreachable!(),
        })
        .await??;
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
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
    if let AfkCommand::Delegate { session, .. }
    | AfkCommand::Audit { session }
    | AfkCommand::AllowBulk { session } = &args.command
    {
        let storage = Storage::open_unwatched(profile)?;
        let (mut rows, _) = storage.load_with_groups()?;
        for row in &mut rows {
            row.source_profile = profile.to_owned();
        }
        let instance = super::resolve_session(session, &rows)?.clone();
        let result = tokio::task::spawn_blocking(move || match args.command {
            AfkCommand::Delegate {
                minutes,
                grant,
                usage_guard,
                ..
            } => crate::session::afk::delegation::delegate(
                &instance,
                minutes,
                &grant,
                usage_guard.as_deref(),
            ),
            AfkCommand::Audit { .. } => crate::session::afk::delegation::audit(&instance),
            AfkCommand::AllowBulk { .. } => crate::session::afk::batch::allow_bulk(&instance),
            _ => unreachable!(),
        })
        .await??;
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }
    let (identifier, operation) = match args.command {
        AfkCommand::On { session, minutes } => (session, Operation::On { minutes }),
        AfkCommand::Off { session } | AfkCommand::OptOut { session } => (session, Operation::Off),
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
    #[derive(clap::Parser)]
    struct InspectionArgs {
        #[command(flatten)]
        afk: super::AfkArgs,
    }
    #[test]
    fn review_export_bypass_startup_but_annotation_is_deliberate() {
        let target = "default/orphan/00000000-0000-4000-8000-000000000001";
        for args in [
            vec!["afk", "review", "--select", target],
            vec![
                "afk",
                "export",
                "--select",
                target,
                "--format",
                "markdown",
                "--output",
                "private/review.md",
            ],
        ] {
            assert!(InspectionArgs::try_parse_from(args)
                .unwrap()
                .afk
                .is_inspection());
        }
        assert!(
            !InspectionArgs::try_parse_from(["afk", "annotate", "--request", "note.json"])
                .unwrap()
                .afk
                .is_inspection()
        );
        for args in [
            vec!["afk", "review"],
            vec!["afk", "review", "--select", "title"],
            vec!["afk", "export", "--select", target, "--format", "json"],
        ] {
            assert!(InspectionArgs::try_parse_from(args).is_err());
        }
    }
    #[test]
    fn batch_requires_explicit_preview_files_and_reviewed_apply_digest() {
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

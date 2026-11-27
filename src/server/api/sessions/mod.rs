//! Session CRUD, ensure-* lifecycle endpoints, and per-file diff handlers.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};

#[cfg(test)]
use crate::daemon::{AttachProjectBody, RenameSessionBody, SetWorktreeNameBody};
use crate::daemon::{
    CleanupDefaults, ContextResumeAvailability, ContextResumeIndeterminateReason,
    ContextResumeUnavailableReason, CreateSessionBody, DeleteSessionBody, ListSessionsQuery,
    PendingApproval, PlanSummary, PurgeOutcome, SessionResponse, TrashSessionBody, Tristate,
    UpdateArchiveBody, UpdateColorBody, UpdateDiffBaseBody, UpdateFavoriteBody, UpdateGroupBody,
    UpdateNotificationsBody, UpdatePinBody, UpdateSnoozeBody, UpdateUnreadBody,
    WorkspaceRepoSummary,
};

use crate::git::error::GitError;
use crate::session::config::SessionConfig;
use crate::session::{
    duplicate_session_error, is_duplicate_session, Instance, LifecycleOperation, Status, Storage,
    TerminalContextResume,
};

use super::validate_no_shell_injection;
use super::AppState;
use super::{api_error, bare_not_found, session_not_found, validate_display_label};

mod artifacts;
mod cli_create;
mod create;
mod delete;
mod diff;
mod ensure;
mod file_edit;
mod lifecycle;
mod list;
mod model;
mod rename;
mod research;
mod search;
mod send;
mod tailored;
mod update;
mod usage;
mod workshop;

pub use artifacts::*;
pub(crate) use cli_create::resolve_cli_parent;
pub use create::*;
pub use delete::*;
pub use diff::*;
pub use ensure::*;
pub use file_edit::*;
pub(crate) use lifecycle::recovery_restart;
pub use lifecycle::*;
pub use list::*;
pub(crate) use model::session_response_from_instance;
use model::*;
pub use rename::*;
pub use research::*;
pub use search::*;
pub use send::*;
pub use tailored::*;
pub use update::*;
pub use usage::*;
pub use workshop::*;

/// Shell-metacharacter gate for free-form spawn fields other than
/// `extra_args`, which goes through [`normalize_extra_args`].
///
/// `command_override` is deliberately absent: it is an `argv[0]` program path
/// rather than shell input, and the create route gates it on the agent
/// registry and the ACP identity check instead (#7), never on metacharacters.
pub(super) fn validate_shell_fields(checks: &[(&str, &str)]) -> Result<(), String> {
    checks
        .iter()
        .find_map(|(value, name)| validate_no_shell_injection(value, name).err())
        .map_or(Ok(()), Err)
}

/// Splits `extra_args` into words and returns them re-quoted for the spawn
/// shell. Callers other than the local Unix owner may not put shell
/// metacharacters inside a word; quoting between words is not checked.
pub(super) fn normalize_extra_args(raw: &str, local_owner: bool) -> Result<String, String> {
    let args = shell_words::split(raw).map_err(|error| format!("Invalid extra_args: {error}"))?;
    if !local_owner {
        for arg in &args {
            validate_no_shell_injection(arg, "extra_args")?;
        }
    }
    Ok(shell_words::join(args))
}

#[cfg(test)]
mod tests;

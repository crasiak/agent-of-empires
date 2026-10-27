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

use crate::daemon::{
    CleanupDefaults, ContextResumeAvailability, ContextResumeIndeterminateReason,
    ContextResumeUnavailableReason, ListSessionsQuery, PendingApproval, PlanSummary,
    SessionResponse, SessionsEnvelope, WorkspaceRepoSummary,
};

use crate::git::error::GitError;
use crate::session::config::SessionConfig;
use crate::session::{
    duplicate_session_error, is_duplicate_session, EnsureReadyError, EnsureReadyOutcome, Instance,
    LifecycleOperation, Status, Storage, TerminalContextResume,
};

use crate::server::push::publish_status_change;

use super::validate_no_shell_injection;
use super::AppState;
use super::{api_error, bare_not_found, find_instance, session_not_found, validate_display_label};

mod artifacts;
mod create;
mod delete;
mod diff;
mod ensure;
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
pub use create::*;
pub use delete::*;
pub use diff::*;
pub use ensure::*;
pub use lifecycle::*;
pub use list::*;
use model::*;
pub use rename::*;
pub use research::*;
pub use search::*;
pub use send::*;
pub use tailored::*;
pub use update::*;
pub use usage::*;
pub use workshop::*;

#[cfg(test)]
mod tests;

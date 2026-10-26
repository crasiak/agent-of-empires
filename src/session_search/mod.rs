//! Client for the `session-search` CLI, free of TUI types.

pub mod group;
pub mod query;
pub mod target;

pub use group::{group, Row};
pub use query::{parse, Client, Hit, QueryError, Request, Resume, Scope, Session, Snippet};
pub use target::{profile_homes, resolve, ProfileHomes, ResumeSpec, Target};

//! Client for the `session-search` CLI, free of TUI types.

pub mod group;
pub mod query;

pub use group::{group, Row};
pub use query::{parse, Client, Hit, QueryError, Request, Resume, Scope, Session, Snippet};

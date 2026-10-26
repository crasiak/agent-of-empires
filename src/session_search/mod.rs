//! Client for the `session-search` CLI, free of TUI types.

pub mod query;

pub use query::{parse, Client, Hit, QueryError, Request, Resume, Scope, Session, Snippet};

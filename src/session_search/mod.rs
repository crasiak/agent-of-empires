//! Client for the `session-search` CLI, free of TUI types.

pub mod context;
pub mod group;
pub mod query;
pub mod resume;
pub mod supervisor;
pub mod target;

pub use group::{group, Row};
pub use query::{parse, Client, Hit, QueryError, Request, Resume, Scope, Session, Snippet};
pub use resume::instance;
pub use target::{profile_homes, resolve, ProfileHomes, ResumeSpec, Target};

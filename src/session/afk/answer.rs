//! Menu recognition and the AFK answer engine (S-A). Pure decisions; delivery lives in the extension.
pub mod decide;
pub mod formats;
pub mod log;
pub mod menu;
pub mod rules;
pub mod run;
pub mod scope;
#[cfg(test)]
mod tests;

pub use decide::{decide, Check, CheckResult, Decision, Policy, Proposal, RunContext};
pub use formats::Formats;
pub use menu::{Kind, Menu, MenuInput, Role, RoleWords, Source};
pub use rules::Rules;

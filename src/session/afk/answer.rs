//! Menu recognition and the AFK answer engine (S-A). Pure decisions; delivery lives in the extension.
pub mod formats;
pub mod menu;
#[cfg(test)]
mod tests;

pub use formats::Formats;
pub use menu::{Kind, Menu, MenuInput, Role, RoleWords, Source};

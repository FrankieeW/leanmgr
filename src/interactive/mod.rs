//! Interactive assistant for cache-management decisions.
//!
//! The assistant has two entry paths: a keyboard-driven dashboard when
//! stdin/stdout are a TTY, and a numbered pipe-mode menu otherwise.
//! Both paths reuse the same action renderers (see `actions.rs`) and
//! the same fleet summary (see `summary.rs`).

mod actions;
mod dashboard;
mod helpers;
mod keyboard;
mod session;
mod summary;

#[cfg(test)]
mod tests;

pub use session::{interact_command, run_session};
pub use summary::{FleetSummary, summarize_fleet};

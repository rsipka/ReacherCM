//! WireGuard tunnels inside the app, used as a session's route: a session
//! (or one of its addresses) set to go through a WireGuard profile connects
//! through [`dial`] instead of a plain TCP socket. See [`stack`] for how the
//! tunnel works and [`manager`] for which ones are running.

pub mod config;
pub mod manager;
pub mod stack;
#[cfg(test)]
mod tests;

pub use manager::dial;

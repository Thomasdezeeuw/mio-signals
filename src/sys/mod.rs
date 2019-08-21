//! Platform dependent implementation of Signals.

#[cfg(unix)]
mod unix;

#[cfg(unix)]
pub use self::unix::Signals;

// TODO: add Windows implementation.

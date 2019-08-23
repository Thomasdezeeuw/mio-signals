//! Platform dependent implementation of Signals.

use crate::Signal;

#[cfg(any(
    target_os = "bitrig",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "ios",
    target_os = "macos",
    target_os = "netbsd",
    target_os = "openbsd"
))]
mod kqueue;

#[cfg(any(
    target_os = "bitrig",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "ios",
    target_os = "macos",
    target_os = "netbsd",
    target_os = "openbsd"
))]
pub use self::kqueue::Signals;

#[cfg(any(target_os = "linux", target_os = "android"))]
mod signalfd;

#[cfg(any(target_os = "linux", target_os = "android"))]
pub use self::signalfd::Signals;

// TODO: add Windows implementation.

/// Convert a `signal` into a Unix signal.
fn raw_signal(signal: Signal) -> libc::c_int {
    match signal {
        Signal::Interrupt => libc::SIGINT,
        Signal::Quit => libc::SIGQUIT,
        Signal::Terminate => libc::SIGTERM,
    }
}
/// Convert a raw Unix signal into a signal.
fn from_raw_signal(raw_signal: libc::c_int) -> Option<Signal> {
    match raw_signal {
        libc::SIGINT => Some(Signal::Interrupt),
        libc::SIGQUIT => Some(Signal::Quit),
        libc::SIGTERM => Some(Signal::Terminate),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::Signal;

    use super::{from_raw_signal, raw_signal};

    #[test]
    fn test_from_raw_signal() {
        assert_eq!(from_raw_signal(libc::SIGINT), Some(Signal::Interrupt));
        assert_eq!(from_raw_signal(libc::SIGQUIT), Some(Signal::Quit));
        assert_eq!(from_raw_signal(libc::SIGTERM), Some(Signal::Terminate));

        // Unsupported signals.
        assert_eq!(from_raw_signal(libc::SIGSTOP), None);
    }

    #[test]
    fn test_raw_signal() {
        assert_eq!(raw_signal(Signal::Interrupt), libc::SIGINT);
        assert_eq!(raw_signal(Signal::Quit), libc::SIGQUIT);
        assert_eq!(raw_signal(Signal::Terminate), libc::SIGTERM);
    }

    #[test]
    fn raw_signal_round_trip() {
        assert_eq!(
            raw_signal(from_raw_signal(libc::SIGINT).unwrap()),
            libc::SIGINT
        );
        assert_eq!(
            raw_signal(from_raw_signal(libc::SIGQUIT).unwrap()),
            libc::SIGQUIT
        );
        assert_eq!(
            raw_signal(from_raw_signal(libc::SIGTERM).unwrap()),
            libc::SIGTERM
        );
    }
}

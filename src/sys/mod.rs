//! Platform dependent implementation of Signals.

use crate::Signal;

#[cfg(any(
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "ios",
    target_os = "macos",
    target_os = "netbsd",
    target_os = "openbsd"
))]
mod kqueue;

#[cfg(any(
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

#[cfg(unix)]
pub fn send_signal(pid: u32, signal: Signal) -> std::io::Result<()> {
    if unsafe { libc::kill(pid as libc::pid_t, raw_signal(signal)) } != 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

// TODO: add Windows implementation.

/// Convert a `signal` into a Unix signal.
fn raw_signal(signal: Signal) -> libc::c_int {
    match signal {
        Signal::Interrupt => libc::SIGINT,
        Signal::Quit => libc::SIGQUIT,
        Signal::Terminate => libc::SIGTERM,
        Signal::User1 => libc::SIGUSR1,
        Signal::User2 => libc::SIGUSR2,
    }
}

/// Convert a raw Unix signal into a signal.
fn from_raw_signal(raw_signal: libc::c_int) -> Option<Signal> {
    match raw_signal {
        libc::SIGINT => Some(Signal::Interrupt),
        libc::SIGQUIT => Some(Signal::Quit),
        libc::SIGTERM => Some(Signal::Terminate),
        libc::SIGUSR1 => Some(Signal::User1),
        libc::SIGUSR2 => Some(Signal::User2),
        _ => None,
    }
}

#[test]
fn test_from_raw_signal() {
    assert_eq!(from_raw_signal(libc::SIGINT), Some(Signal::Interrupt));
    assert_eq!(from_raw_signal(libc::SIGQUIT), Some(Signal::Quit));
    assert_eq!(from_raw_signal(libc::SIGTERM), Some(Signal::Terminate));
    assert_eq!(from_raw_signal(libc::SIGUSR1), Some(Signal::User1));
    assert_eq!(from_raw_signal(libc::SIGUSR2), Some(Signal::User2));

    // Unsupported signals.
    assert_eq!(from_raw_signal(libc::SIGSTOP), None);
}

#[test]
fn test_raw_signal() {
    assert_eq!(raw_signal(Signal::Interrupt), libc::SIGINT);
    assert_eq!(raw_signal(Signal::Quit), libc::SIGQUIT);
    assert_eq!(raw_signal(Signal::Terminate), libc::SIGTERM);
    assert_eq!(raw_signal(Signal::User1), libc::SIGUSR1);
    assert_eq!(raw_signal(Signal::User2), libc::SIGUSR2);
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
    assert_eq!(
        raw_signal(from_raw_signal(libc::SIGUSR1).unwrap()),
        libc::SIGUSR1
    );
    assert_eq!(
        raw_signal(from_raw_signal(libc::SIGUSR2).unwrap()),
        libc::SIGUSR2
    );
}

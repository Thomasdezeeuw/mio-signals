//! This tests if the signals handlers are properly reset.
//!
//! # Notes
//!
//! This needs to run on its own and thus has its own file.

use mio_signals::Signal;

#[cfg(any(
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "ios",
    target_os = "macos",
    target_os = "netbsd",
    target_os = "openbsd"
))]
mod kqueue {
    use std::mem::MaybeUninit;
    use std::{io, ptr};

    use mio_signals::{SignalSet, Signals};

    use super::raw_signal;

    type SigActions = [MaybeUninit<libc::sigaction>; SignalSet::all().len()];

    #[test]
    fn cleanup() {
        // Before `Signals` is created.
        let mut original_actions: SigActions = unsafe { MaybeUninit::uninit().assume_init() };
        let original_actions = get_sigactions(&mut original_actions).unwrap();

        // After `Signals` is created.
        let signals = Signals::new(SignalSet::all()).unwrap();
        let mut ignored_actions: SigActions = unsafe { MaybeUninit::uninit().assume_init() };
        let ignored_actions = get_sigactions(&mut ignored_actions).unwrap();

        for (signal, ignored) in SignalSet::all().into_iter().zip(ignored_actions) {
            if ignored.sa_sigaction != libc::SIG_IGN {
                panic!(
                    "sigaction.sa_sigaction for signal: {:?} is not ignored, but {}",
                    signal, ignored.sa_sigaction
                );
            }
        }

        // After `Signals` is dropped.
        drop(signals);
        let mut cleaned_actions: SigActions = unsafe { MaybeUninit::uninit().assume_init() };
        let cleaned_actions = get_sigactions(&mut cleaned_actions).unwrap();

        for ((signal, original), cleaned) in SignalSet::all()
            .into_iter()
            .zip(original_actions)
            .zip(cleaned_actions)
        {
            if original.sa_sigaction != cleaned.sa_sigaction {
                panic!(
                    "sigaction.sa_sigaction is different for signal {:?}, original: {}, cleaned: {}",
                    signal, original.sa_sigaction, cleaned.sa_sigaction
                );
            }
        }
    }

    fn get_sigactions(actions: &mut SigActions) -> io::Result<&[libc::sigaction]> {
        for (signal, old_action) in SignalSet::all().into_iter().zip(actions.iter_mut()) {
            if unsafe {
                libc::sigaction(raw_signal(signal), ptr::null_mut(), old_action.as_mut_ptr())
            } == -1
            {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(unsafe { &*(actions as *const [_] as *const [_]) })
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod signalfd {
    use std::mem::MaybeUninit;
    use std::{io, ptr};

    use mio_signals::{Signal, SignalSet, Signals};

    use super::raw_signal;

    #[test]
    fn cleanup() {
        // Before `Signals` is created.
        let original_set = get_blocked_set().unwrap();
        for signal in SignalSet::all() {
            assert!(!is_in_set(&original_set, signal));
        }

        // After `Signals` is created.
        let signals = Signals::new(SignalSet::all()).unwrap();
        let blocked_set = get_blocked_set().unwrap();

        for signal in SignalSet::all() {
            assert!(
                is_in_set(&blocked_set, signal),
                "missing signal {:?} from blocked set",
                signal
            );
        }

        // After `Signals` is dropped.
        drop(signals);
        let cleaned_set = get_blocked_set().unwrap();
        for signal in SignalSet::all() {
            assert!(!is_in_set(&cleaned_set, signal));
        }
    }

    fn get_blocked_set() -> io::Result<libc::sigset_t> {
        let mut old_set: MaybeUninit<libc::sigset_t> = MaybeUninit::uninit();
        if unsafe { libc::sigprocmask(0, ptr::null_mut(), old_set.as_mut_ptr()) } == -1 {
            Err(io::Error::last_os_error())
        } else {
            // This is safe as `sigprocmask` fills it for us.
            Ok(unsafe { old_set.assume_init() })
        }
    }

    fn is_in_set(set: &libc::sigset_t, signal: Signal) -> bool {
        match unsafe { libc::sigismember(set, raw_signal(signal)) } {
            1 => true,
            0 => false,
            -1 => panic!("unexpected error: {}", io::Error::last_os_error()),
            _ => unreachable!(),
        }
    }
}

// Keep in sync with `mio_signals::sys::raw_signal`.
fn raw_signal(signal: Signal) -> libc::c_int {
    match signal {
        Signal::Interrupt => libc::SIGINT,
        Signal::Quit => libc::SIGQUIT,
        Signal::Terminate => libc::SIGTERM,
    }
}

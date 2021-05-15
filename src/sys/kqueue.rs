use std::mem::MaybeUninit;
use std::os::unix::io::RawFd;
use std::{io, ptr};

use log::error;
use mio::unix::SourceFd;
use mio::{event, Interest, Registry, Token};

use crate::{Signal, SignalSet};

use super::{from_raw_signal, raw_signal};

/// Signaler backed that uses `kqueue(2)`'s `EVFILT_SIGNAL`.
///
/// # Implementation notes
///
/// We crate a new `kqueue` which we register with the `kqueue` in `Poll`, so we
/// can received signals by calling `receive` instead of returning them as
/// `Event`s when calling `Poll::poll` to match the API provided by the
/// `signalfd` implementation.
///
/// We set the signal handler to ignore the signal (not blocking them like in
/// the signalfd implementation) to ensure the signal doesn't grow endlessly.
#[derive(Debug)]
pub struct Signals {
    /// `kqueue(2)` file descriptor.
    kq: RawFd,
    /// All signals this is listening for, used in resetting the signal handlers.
    signals: SignalSet,
}

impl Signals {
    pub fn new(signals: SignalSet) -> io::Result<Signals> {
        new_kqueue()
            .map(|kq| Signals { kq, signals })
            .and_then(|kq| register_signals(kq.kq, signals).map(|()| kq))
            .and_then(|kq| ignore_signals(signals).map(|()| kq))
    }

    pub fn receive(&mut self) -> io::Result<Option<Signal>> {
        let mut kevent: MaybeUninit<libc::kevent> = MaybeUninit::uninit();
        // No blocking.
        let timeout = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };

        let n_events =
            unsafe { libc::kevent(self.kq, ptr::null(), 0, kevent.as_mut_ptr(), 1, &timeout) };
        match n_events {
            -1 => Err(io::Error::last_os_error()),
            0 => Ok(None), // No signals.
            1 => {
                // This is safe because `kevent` ensures that the event is
                // initialised.
                let kevent = unsafe { kevent.assume_init() };
                // Should never happen, but just in case.
                let filter = kevent.filter; // Can't create ref to packed struct.
                debug_assert_eq!(filter, libc::EVFILT_SIGNAL);
                // This should never return `None` as we control the signals we
                // register for, which is always defined in terms of `Signal`.
                Ok(from_raw_signal(kevent.ident as libc::c_int))
            }
            _ => unreachable!("unexpected number of events"),
        }
    }
}

fn new_kqueue() -> io::Result<RawFd> {
    let kq = unsafe { libc::kqueue() };
    if kq == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(kq)
    }
}

fn register_signals(kq: RawFd, signals: SignalSet) -> io::Result<()> {
    // For each signal create an kevent to indicate we want events for
    // those signals.
    let mut changes: [MaybeUninit<libc::kevent>; SignalSet::all().len()] =
        [MaybeUninit::uninit(); SignalSet::all().len()];
    let mut n_changes = 0;
    for signal in signals {
        changes[n_changes] = MaybeUninit::new(libc::kevent {
            ident: raw_signal(signal) as libc::uintptr_t,
            filter: libc::EVFILT_SIGNAL,
            flags: libc::EV_ADD,
            fflags: 0,
            data: 0,
            udata: 0 as _,
        });
        n_changes += 1;
    }

    let ok = unsafe {
        libc::kevent(
            kq,
            changes[0].as_ptr(),
            n_changes as _,
            ptr::null_mut(),
            0,
            ptr::null(),
        )
    };
    if ok == -1 {
        // EINTR is the only error that we can handle, but according to
        // the man page of FreeBSD: "When kevent() call fails with EINTR
        // error, all changes in the changelist have been applied", so
        // we're done.
        //
        // EOPNOTSUPP (NetBSD only),
        // EACCESS, EFAULT, ENOMEM: can't handle.
        //
        // EBADF, EINVAL,
        // ENOENT, and ESRCH: all have to do with invalid arguments,
        //                    which shouldn't happen.
        let err = io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::EINTR) => Ok(()),
            _ => Err(err),
        }
    } else {
        Ok(())
    }
}

/// Ignore all signals in the `signals` set.
fn ignore_signals(signals: SignalSet) -> io::Result<()> {
    sigaction(signals, libc::SIG_IGN)
}

/// Inverse of `ignore_signals`, resetting all signal handlers to the default.
fn unignore_signals(signals: SignalSet) -> io::Result<()> {
    sigaction(signals, libc::SIG_DFL)
}

/// Call `sigaction` for each signal in `signals`, using `action` as signal
/// handler.
fn sigaction(signals: SignalSet, action: libc::sighandler_t) -> io::Result<()> {
    let action = libc::sigaction {
        sa_sigaction: action,
        sa_mask: empty_sigset()?,
        sa_flags: 0,
    };
    for signal in signals {
        if unsafe { libc::sigaction(raw_signal(signal), &action, ptr::null_mut()) } == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Create an empty `sigset_t`.
fn empty_sigset() -> io::Result<libc::sigset_t> {
    let mut set: MaybeUninit<libc::sigset_t> = MaybeUninit::uninit();
    if unsafe { libc::sigemptyset(set.as_mut_ptr()) } == -1 {
        Err(io::Error::last_os_error())
    } else {
        // This is safe because `sigemptyset` ensures `set` is initialised.
        Ok(unsafe { set.assume_init() })
    }
}

impl event::Source for Signals {
    fn register(
        &mut self,
        registry: &Registry,
        token: Token,
        interests: Interest,
    ) -> io::Result<()> {
        SourceFd(&self.kq).register(registry, token, interests)
    }

    fn reregister(
        &mut self,
        registry: &Registry,
        token: Token,
        interests: Interest,
    ) -> io::Result<()> {
        SourceFd(&self.kq).reregister(registry, token, interests)
    }

    fn deregister(&mut self, registry: &Registry) -> io::Result<()> {
        SourceFd(&self.kq).deregister(registry)
    }
}

impl Drop for Signals {
    fn drop(&mut self) {
        // Reverse the ignoring of signals.
        if let Err(err) = unignore_signals(self.signals) {
            error!("error resetting signal action: {}", err);
        }

        if unsafe { libc::close(self.kq) } == -1 {
            // Possible errors:
            // - EBADF, EIO: can't recover.
            // - EINTR: could try again but we're can't be sure if the file
            //          descriptor was closed or not, so to be safe we don't
            //          close it again.
            let err = io::Error::last_os_error();
            error!("error closing Signals: {}", err);
        }
    }
}

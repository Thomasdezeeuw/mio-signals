use std::mem::MaybeUninit;
use std::os::unix::io::RawFd;
use std::{io, ptr};

use log::error;
use mio::unix::SourceFd;
use mio::{event, Interests, Registry, Token};

use crate::{Signal, SignalSet};

use super::{from_raw_signal, raw_signal};

/// Signaler backed by kqueue (`EVFILT_SIGNAL`).
///
/// We crate a new kqueue which we register with the kqueue in `Poll`, so we
/// can received signals by calling `receive` instead of returning them when
/// calling `Poll::poll` to match the API provided by `signalfd`.
#[derive(Debug)]
pub struct Signals {
    // Separate from the associated kqueue in `Poll`.
    kq: RawFd,
}

impl Signals {
    pub fn new(signals: SignalSet) -> io::Result<Signals> {
        // Create our own kqueue.
        let kq = unsafe { libc::kqueue() };
        let kq = if kq == -1 {
            Err(io::Error::last_os_error())
        } else {
            Ok(Signals { kq })
        }?;

        // For each signal create an kevent to indicate we want events for
        // those signals.
        let mut changes: [MaybeUninit<libc::kevent>; SignalSet::all().len()] = [
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
        ];
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

        // Register the event signals with our kqueue.
        let ok = unsafe {
            libc::kevent(
                kq.kq,
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
                Some(libc::EINTR) => (),
                _ => return Err(err),
            }
        }

        ignore_signals(signals).map(|()| kq)
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
                if kevent.filter == libc::EVFILT_SIGNAL {
                    // This should never return `None` as we control the
                    // signals we register for, which are defined in terms
                    // of `Signal`.
                    Ok(from_raw_signal(kevent.ident as libc::c_int))
                } else {
                    // Should never happen, but just in case.
                    Ok(None)
                }
            }
            _ => unreachable!("unexpected number of events"),
        }
    }
}

/// Ignore all signals in the `signals` set.
fn ignore_signals(signals: SignalSet) -> io::Result<()> {
    // Most OSes use `sigset_t` as mask, Darwin disagrees and uses an `int`.
    let mask = {
        #[cfg(any(
            target_os = "bitrig",
            target_os = "dragonfly",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd",
        ))]
        {
            let mut set: MaybeUninit<libc::sigset_t> = MaybeUninit::uninit();
            if unsafe { libc::sigemptyset(set.as_mut_ptr()) } == -1 {
                return Err(io::Error::last_os_error());
            }
            // This is safe because `sigemptyset` ensures `set` is initialised.
            unsafe { set.assume_init() }
        }
        #[cfg(any(target_os = "ios", target_os = "macos"))]
        {
            0
        }
    };
    let action = libc::sigaction {
        sa_sigaction: libc::SIG_IGN,
        sa_mask: mask,
        sa_flags: 0,
        #[cfg(any(target_os = "android", target_os = "linux"))]
        sa_restorer: None,
    };
    for signal in signals {
        if unsafe { libc::sigaction(raw_signal(signal), &action, ptr::null_mut()) } == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

impl event::Source for Signals {
    fn register(
        &self,
        registry: &Registry,
        token: Token,
        interests: Interests,
    ) -> io::Result<()> {
        SourceFd(&self.kq).register(registry, token, interests)
    }

    fn reregister(
        &self,
        registry: &Registry,
        token: Token,
        interests: Interests,
    ) -> io::Result<()> {
        SourceFd(&self.kq).reregister(registry, token, interests)
    }

    fn deregister(&self, registry: &Registry) -> io::Result<()> {
        SourceFd(&self.kq).deregister(registry)
    }
}

impl Drop for Signals {
    fn drop(&mut self) {
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

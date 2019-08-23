use std::mem::MaybeUninit;
use std::{io, ptr};

use crate::{Signal, SignalSet};

#[cfg(any(target_os = "linux", target_os = "android"))]
mod signalfd {
    use std::io;
    use std::mem::{size_of, MaybeUninit};
    use std::os::unix::io::RawFd;

    use log::error;
    use mio::unix::SourceFd;
    use mio::{event, Interests, Registry, Token};

    use crate::{Signal, SignalSet};

    use super::{block_signals, create_sigset, from_raw_signal};

    /// Signaler backed by `signalfd`.
    #[derive(Debug)]
    pub struct Signals {
        fd: RawFd,
    }

    impl Signals {
        pub fn new(signals: SignalSet) -> io::Result<Signals> {
            // Create a mask for all signal we want to handle.
            create_sigset(signals)
                .and_then(|set| {
                    // Create a new signal file descriptor.
                    let fd =
                        unsafe { libc::signalfd(-1, &set, libc::SFD_CLOEXEC | libc::SFD_NONBLOCK) };
                    if fd == -1 {
                        Err(io::Error::last_os_error())
                    } else {
                        Ok((Signals { fd }, set))
                    }
                })
                // Block signals from interrupting the process.
                .and_then(|(fd, set)| block_signals(set).map(|()| fd))
        }

        pub fn receive(&mut self) -> io::Result<Option<Signal>> {
            let mut info: MaybeUninit<libc::signalfd_siginfo> = MaybeUninit::uninit();

            loop {
                let n = unsafe {
                    libc::read(
                        self.fd,
                        info.as_mut_ptr() as *mut _,
                        size_of::<libc::signalfd_siginfo>(),
                    )
                };

                const INFO_SIZE: isize = size_of::<libc::signalfd_siginfo>() as isize;
                match n {
                    -1 => match io::Error::last_os_error() {
                        ref err if err.kind() == io::ErrorKind::WouldBlock => return Ok(None),
                        ref err if err.kind() == io::ErrorKind::Interrupted => continue,
                        err => return Err(err),
                    },
                    INFO_SIZE => {
                        // This is safe because we just read into it.
                        let info = unsafe { info.assume_init() };
                        return Ok(from_raw_signal(info.ssi_signo as libc::c_int));
                    }
                    _ => unreachable!("read an incorrect amount of bytes from signalfd"),
                }
            }
        }
    }

    impl event::Source for Signals {
        fn register(
            &self,
            registry: &Registry,
            token: Token,
            interests: Interests,
        ) -> io::Result<()> {
            SourceFd(&self.fd).register(registry, token, interests)
        }

        fn reregister(
            &self,
            registry: &Registry,
            token: Token,
            interests: Interests,
        ) -> io::Result<()> {
            SourceFd(&self.fd).reregister(registry, token, interests)
        }

        fn deregister(&self, registry: &Registry) -> io::Result<()> {
            SourceFd(&self.fd).deregister(registry)
        }
    }

    impl Drop for Signals {
        fn drop(&mut self) {
            if unsafe { libc::close(self.fd) } == -1 {
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
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub use self::signalfd::Signals;

#[cfg(any(
    target_os = "bitrig",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "ios",
    target_os = "macos",
    target_os = "netbsd",
    target_os = "openbsd"
))]
mod kqueue {
    use std::mem::MaybeUninit;
    use std::os::unix::io::RawFd;
    use std::{io, ptr};

    use log::error;
    use mio::unix::SourceFd;
    use mio::{event, Interests, Registry, Token};

    use crate::{Signal, SignalSet};

    use super::{block_signals, create_sigset, from_raw_signal, raw_signal};

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
                    udata: ptr::null_mut(),
                });
                n_changes += 1;
            }

            // Register the event signals with our kqueue.
            let ok = unsafe {
                libc::kevent(
                    kq.kq,
                    changes[0].as_ptr(),
                    n_changes as libc::c_int,
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

            // Block signals from interrupting the process.
            create_sigset(signals).and_then(block_signals).map(|()| kq)
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
}

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

/// Create a `libc::sigset_t` from `SignalSet`.
fn create_sigset(signals: SignalSet) -> io::Result<libc::sigset_t> {
    let mut set: MaybeUninit<libc::sigset_t> = MaybeUninit::uninit();
    if unsafe { libc::sigemptyset(set.as_mut_ptr()) } == -1 {
        return Err(io::Error::last_os_error());
    }
    // This is safe because `sigemptyset` ensures `set` is initialised.
    let mut set = unsafe { set.assume_init() };
    for signal in signals {
        if unsafe { libc::sigaddset(&mut set, raw_signal(signal)) } == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(set)
}

/// Block all signals in `set`.
fn block_signals(set: libc::sigset_t) -> io::Result<()> {
    if unsafe { libc::sigprocmask(libc::SIG_BLOCK, &set, ptr::null_mut()) } == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
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

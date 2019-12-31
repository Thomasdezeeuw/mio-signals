use std::io::{self, Read};
use std::mem::{size_of, MaybeUninit};
#[cfg(unix)]
use std::os::unix::io::IntoRawFd;
use std::ptr;
use std::sync::atomic::{AtomicI32, Ordering};

use log::error;
use mio::{event, Interest, Registry, Token};
use mio_pipe::{new_pipe, Receiver};

use crate::sys::from_raw_signal;
use crate::{Signal, SignalSet};

type RawFd = libc::c_int;
type AtomicRawFd = AtomicI32;

#[test]
fn assert_raw_fd_same_type() {
    let a: AtomicRawFd = AtomicRawFd::new(0);
    let b: RawFd = 0;
    // This will fail to compile if `AtomicRawFd` and `RawFd` use different types.
    assert_eq!(a.into_inner(), b);
}

type RawSignal = libc::c_int;
type SignalBytes = [u8; 4];

#[test]
fn assert_signal_same_type() {
    // This will fail to compile if the `RawSignal` and `SignalByes` types don't
    // align.
    let a: RawSignal = 0;
    let b: SignalBytes = a.to_ne_bytes();
    assert_eq!(a.to_ne_bytes(), b);
}

// Atomic, writeable file descriptors used by `signal_handler` to write the
// signal to. If these are `-1` they are not set.
static SIGINT_FD: AtomicRawFd = AtomicRawFd::new(-1);
static SIGTERM_FD: AtomicRawFd = AtomicRawFd::new(-1);
#[cfg(not(windows))] // Not supported on Windows.
static SIGQUIT_FD: AtomicRawFd = AtomicRawFd::new(-1);

extern "C" fn signal_handler(raw_signal: RawSignal) {
    let fd = match raw_signal {
        libc::SIGINT => SIGINT_FD.load(Ordering::Relaxed),
        libc::SIGTERM => SIGTERM_FD.load(Ordering::Relaxed),
        #[cfg(unix)]
        libc::SIGQUIT => SIGQUIT_FD.load(Ordering::Relaxed),
        _ => -1,
    };

    // Invalid signal or file descriptor.
    if fd == -1 {
        return;
    }

    // Can't handle the error from here, so we just ignore it.
    let bytes: SignalBytes = raw_signal.to_ne_bytes();
    // FIXME: handle error.
    let _ = unsafe {
        libc::write(
            fd,
            &bytes as *const _ as *const _,
            size_of::<SignalBytes>() as _,
        )
    };
}

/// Signaler backed that uses a `pipe(2)`.
///
/// # Implementation notes
///
/// We create a pipe, registering the reading end with `Poll`. The writing end
/// will be used to write the signal into in the `signal_handler`.
#[derive(Debug)]
pub struct Signals {
    /// Receiving end of the pipe.
    recv: Receiver,
    /// All signals this is listening for, used in resetting the signal handlers.
    signals: SignalSet,
}

impl Signals {
    pub fn new(signals: SignalSet) -> io::Result<Signals> {
        new_pipe().and_then(|(send, recv)| {
            let p = Signals { recv, signals };

            let fd = send.into_raw_fd();
            for signal in signals {
                let signal_fd = match signal {
                    Signal::Interrupt => &SIGINT_FD,
                    #[cfg(unix)]
                    Signal::Quit => &SIGQUIT_FD,
                    #[cfg(windows)]
                    Signal::Quit => continue, // Not supported on Windows.
                    Signal::Terminate => &SIGTERM_FD,
                };
                signal_fd.store(fd, Ordering::Relaxed);
            }

            set_signal_handler(signals).map(|()| p)
        })
    }

    pub fn receive(&mut self) -> io::Result<Option<Signal>> {
        const SIGNAL_SIZE: usize = size_of::<SignalBytes>();
        let mut signal: SignalBytes = (-1 as RawSignal).to_ne_bytes();

        loop {
            // FIXME: reading order in reversed in tests.
            match self.recv.read(&mut signal) {
                Ok(SIGNAL_SIZE) => return Ok(from_raw_signal(RawSignal::from_ne_bytes(signal))),
                Ok(_) => unreachable!("read an incorrect amount of bytes from pipe"),
                Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => return Ok(None),
                Err(ref err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(err),
            }
        }
    }
}

// TODO: DRY this with the kqueue code.

/// Set the signal handler for all signals in `signals` to call
/// `signal_handler`.
fn set_signal_handler(signals: SignalSet) -> io::Result<()> {
    sigaction(signals, signal_handler as libc::sighandler_t)
}

/// Inverse of `set_signal_handler`, resetting all signal handlers to the default.
fn reset_signal_handler(signals: SignalSet) -> io::Result<()> {
    sigaction(signals, libc::SIG_DFL)
}

/// Call `sigaction` for each signal in `signals`, using `action` as signal
/// handler.
fn sigaction(signals: SignalSet, action: libc::sighandler_t) -> io::Result<()> {
    let action = libc::sigaction {
        sa_sigaction: action,
        sa_mask: empty_sigset()?,
        sa_flags: libc::SA_RESTART,
    };
    for raw_signal in signals.into_iter().filter_map(raw_signal) {
        if unsafe { libc::sigaction(raw_signal, &action, ptr::null_mut()) } == -1 {
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

/// Convert a `signal` into a Unix signal.
fn raw_signal(signal: Signal) -> Option<RawSignal> {
    match signal {
        Signal::Interrupt => Some(libc::SIGINT),
        #[cfg(unix)]
        Signal::Quit => Some(libc::SIGQUIT),
        #[cfg(windows)]
        Signal::Quit => None, // Not supported on Windows.
        Signal::Terminate => Some(libc::SIGTERM),
    }
}

impl event::Source for Signals {
    fn register(
        &mut self,
        registry: &Registry,
        token: Token,
        interests: Interest,
    ) -> io::Result<()> {
        self.recv.register(registry, token, interests)
    }

    fn reregister(
        &mut self,
        registry: &Registry,
        token: Token,
        interests: Interest,
    ) -> io::Result<()> {
        self.recv.reregister(registry, token, interests)
    }

    fn deregister(&mut self, registry: &Registry) -> io::Result<()> {
        self.recv.deregister(registry)
    }
}

impl Drop for Signals {
    fn drop(&mut self) {
        // Reset the signal handler.
        if let Err(err) = reset_signal_handler(self.signals) {
            error!("error resetting signal handler: {}", err);
        }

        // Reset the signal file descriptors.
        let mut fd: RawFd = -1;
        for signal in self.signals {
            let signal_fd = match signal {
                Signal::Interrupt => &SIGINT_FD,
                #[cfg(unix)]
                Signal::Quit => &SIGQUIT_FD,
                #[cfg(windows)]
                Signal::Quit => continue, // Not supported on Windows.
                Signal::Terminate => &SIGTERM_FD,
            };
            let recv_fd = signal_fd.swap(-1, Ordering::Relaxed);
            if fd == -1 {
                fd = recv_fd;
            } else {
                debug_assert_eq!(fd, recv_fd);
            }
        }

        // Finally close the sending end of the pipe.
        if unsafe { libc::close(fd) } == -1 {
            // Possible errors:
            // - EBADF, EIO: can't recover.
            // - EINTR: could try again but we're can't be sure if the file
            //          descriptor was closed or not, so to be safe we don't
            //          close it again.
            let err = io::Error::last_os_error();
            error!("error closing unix pipe: {}", err);
        }
    }
}

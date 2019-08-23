use std::mem::{size_of, MaybeUninit};
use std::os::unix::io::RawFd;
use std::{io, ptr};

use log::error;
use mio::unix::SourceFd;
use mio::{event, Interests, Registry, Token};

use crate::{Signal, SignalSet};

use super::{from_raw_signal, raw_signal};

/// Signaler backed by `signalfd(2)`.
///
/// # Implementation notes
///
/// We create a `signalfd` which we register with the `epoll` in `Poll`. This
/// will have a reference to the signal queue from which we can read (using
/// `read(2)`). However the regular signal handler is still invoked, to prevent
/// this we block signals (see `block_signals`). This is fine because reading
/// from `signalfd` will remove them from the queue, so we don't have an
/// endlessly growing signal queue.
///
/// We can't ignore the signal using `SIG_IGN`, like we do in the kqueue
/// implementation, because then the signals don't end up in our `signalfd`
/// either.
#[derive(Debug)]
pub struct Signals {
    /// `signalfd(2)` file descriptor.
    fd: RawFd,
}

impl Signals {
    pub fn new(signals: SignalSet) -> io::Result<Signals> {
        create_sigset(signals)
            .and_then(|set| new_signalfd(&set).map(|fd| (Signals { fd }, set)))
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

fn new_signalfd(set: &libc::sigset_t) -> io::Result<RawFd> {
    let fd = unsafe { libc::signalfd(-1, set, libc::SFD_CLOEXEC | libc::SFD_NONBLOCK) };
    if fd == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(fd)
    }
}

/// Block all signals in `set`.
fn block_signals(set: libc::sigset_t) -> io::Result<()> {
    if unsafe { libc::sigprocmask(libc::SIG_BLOCK, &set, ptr::null_mut()) } == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

impl event::Source for Signals {
    fn register(&self, registry: &Registry, token: Token, interests: Interests) -> io::Result<()> {
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

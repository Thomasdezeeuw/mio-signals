use std::mem::{size_of, MaybeUninit};
use std::os::unix::io::RawFd;
use std::{io, ptr};

use log::error;
use mio::unix::SourceFd;
use mio::{event, Interests, Registry, Token};

use crate::{Signal, SignalSet};

use super::{from_raw_signal, raw_signal};

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

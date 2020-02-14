//! Crate for handling signals with Mio.
//!
//! See the [`Signals`] documentation.
//!
//! ## Supported platforms
//!
//! Currently supported platforms:
//!
//! * Android
//! * DragonFly BSD
//! * FreeBSD
//! * Linux
//! * NetBSD
//! * OpenBSD
//! * iOS
//! * macOS
//!
//! The most notable exception in the list is Windows. If you want to contribute
//! a port to Windows please see [issue #4].
//!
//! [issue #4]: https://github.com/Thomasdezeeuw/mio-signals/issues/4

// TODO: #[non_exhaustive] to `Signal`.

#![warn(
    missing_debug_implementations,
    missing_docs,
    rust_2018_idioms,
    unused_qualifications,
    unused_results,
    variant_size_differences
)]
// Disallow warnings when running tests.
#![cfg_attr(test, deny(warnings))]
// Disallow warnings in examples, we want to set a good example after all.
#![doc(test(attr(deny(warnings))))]
// `SignalSet` can never be empty, thus an `is_empty` method doesn't make sense.
#![allow(clippy::len_without_is_empty)]

use std::iter::FusedIterator;
use std::num::NonZeroU8;
use std::ops::BitOr;
use std::{fmt, io};

use mio::{event, Interest, Registry, Token};

mod sys;

/// Notification of process signals.
///
/// # Notes
///
/// On Android and Linux this will block all signals in the signal set given
/// when creating `Signals`, using [`sigprocmask(2)`]. This means that the
/// program is not interrupted, or in any way notified of signal until the
/// assiocated [`Poll`] is [polled].
///
/// On platforms that support [`kqueue(2)`] the signal handler action is set to
/// `SIG_IGN` using [`sigaction(2)`], meaning that all signals will be ignored.
/// Same as on Linux based systems; the program is not interrupted, or in any way
/// notified of signal until the assiocated [`Poll`] is [polled].
///
/// [`sigprocmask(2)`]: http://man7.org/linux/man-pages/man2/sigprocmask.2.html
/// [`Poll`]: mio::Poll
/// [polled]: mio::Poll::poll
/// [`kqueue(2)`]: https://www.freebsd.org/cgi/man.cgi?query=kqueue&sektion=2
/// [`sigaction(2)`]: https://www.freebsd.org/cgi/man.cgi?query=sigaction&sektion=2
///
/// # Implementation notes
///
/// On platforms that support [`kqueue(2)`] this will use the `EVFILT_SIGNAL`
/// event filter. On Android and Linux it uses [`signalfd(2)`].
///
/// [`signalfd(2)`]: http://man7.org/linux/man-pages/man2/signalfd.2.html
///
/// # Examples
/// ```
/// use std::io;
///
/// use mio::{Poll, Events, Interest, Token};
/// use mio_signals::{Signals, Signal, SignalSet};
///
/// const SIGNAL: Token = Token(10);
///
/// fn main() -> io::Result<()> {
///     let mut poll = Poll::new()?;
///     let mut events = Events::with_capacity(128);
///
///     // Create a `Signals` instance that will catch signals for us.
///     let mut signals = Signals::new(SignalSet::all())?;
///     // And register it with our `Poll` instance.
///     poll.registry().register(&mut signals, SIGNAL, Interest::READABLE)?;
///
///     # // Don't want to let the example run for ever.
///     # let awakener = mio::Waker::new(&poll.registry(), Token(20))?;
///     # awakener.wake()?;
///     #
///     loop {
///         poll.poll(&mut events, None)?;
///
///         for event in events.iter() {
///             match event.token() {
///                 // Because we're using edge triggers (default in Mio) we need
///                 // to keep calling `receive` until it returns `Ok(None)`.
///                 SIGNAL => loop {
///                     match signals.receive()? {
///                         Some(Signal::Interrupt) => println!("Got interrupt signal"),
///                         Some(Signal::Terminate) => println!("Got terminate signal"),
///                         Some(Signal::Quit) => println!("Got quit signal"),
///                         None => break,
///                     }
///                 },
/// #               Token(20) => return Ok(()),
///                 _ => println!("Got unexpected event: {:?}", event),
///             }
///         }
///     }
/// }
/// ```
#[derive(Debug)]
pub struct Signals {
    sys: sys::Signals,
}

impl Signals {
    /// Create a new signal notifier.
    pub fn new(signals: SignalSet) -> io::Result<Signals> {
        sys::Signals::new(signals).map(|sys| Signals { sys })
    }

    /// Receive a signal, if any.
    ///
    /// If no signal is available this returns `Ok(None)`.
    pub fn receive(&mut self) -> io::Result<Option<Signal>> {
        self.sys.receive()
    }
}

impl event::Source for Signals {
    fn register(
        &mut self,
        registry: &Registry,
        token: Token,
        interests: Interest,
    ) -> io::Result<()> {
        self.sys.register(registry, token, interests)
    }

    fn reregister(
        &mut self,
        registry: &Registry,
        token: Token,
        interests: Interest,
    ) -> io::Result<()> {
        self.sys.reregister(registry, token, interests)
    }

    fn deregister(&mut self, registry: &Registry) -> io::Result<()> {
        self.sys.deregister(registry)
    }
}

/// Set of [`Signal`]s used in registering signal notifications with [`Signals`].
///
/// # Examples
///
/// ```
/// use mio_signals::{Signal, SignalSet};
///
/// // Signal set can be created by bit-oring (`|`) signals together.
/// let set: SignalSet = Signal::Interrupt | Signal::Quit;
/// assert_eq!(set.len(), 2);
///
/// assert!(set.contains(Signal::Interrupt));
/// assert!(set.contains(Signal::Quit));
/// assert!(!set.contains(Signal::Terminate));
/// assert!(set.contains(Signal::Interrupt | Signal::Quit));
/// ```
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SignalSet(NonZeroU8);

// NOTE: these may never be zero.
const INTERRUPT: u8 = 1;
const QUIT: u8 = 1 << 1;
const TERMINATE: u8 = 1 << 2;

impl SignalSet {
    /// Create a new set with all signals.
    pub const fn all() -> SignalSet {
        SignalSet(unsafe { NonZeroU8::new_unchecked(INTERRUPT | QUIT | TERMINATE) })
    }

    /// Number of signals in the set.
    pub const fn len(self) -> usize {
        self.0.get().count_ones() as usize
    }

    /// Whether or not all signals in `other` are contained within `self`.
    ///
    /// # Notes
    ///
    /// This can also be used with [`Signal`].
    ///
    /// # Examples
    ///
    /// ```
    /// use mio_signals::{Signal, SignalSet};
    ///
    /// let set = SignalSet::all();
    ///
    /// assert!(set.contains(Signal::Interrupt));
    /// assert!(set.contains(Signal::Quit));
    /// assert!(set.contains(Signal::Interrupt | Signal::Quit));
    /// ```
    pub fn contains<S>(self, other: S) -> bool
    where
        S: Into<SignalSet>,
    {
        let other = other.into();
        (self.0.get() & other.0.get()) == other.0.get()
    }
}

impl From<Signal> for SignalSet {
    fn from(signal: Signal) -> Self {
        SignalSet(unsafe {
            NonZeroU8::new_unchecked(match signal {
                Signal::Interrupt => INTERRUPT,
                Signal::Quit => QUIT,
                Signal::Terminate => TERMINATE,
            })
        })
    }
}

impl BitOr for SignalSet {
    type Output = SignalSet;

    fn bitor(self, rhs: Self) -> Self {
        SignalSet(unsafe { NonZeroU8::new_unchecked(self.0.get() | rhs.0.get()) })
    }
}

impl BitOr<Signal> for SignalSet {
    type Output = SignalSet;

    fn bitor(self, rhs: Signal) -> Self {
        self | Into::<SignalSet>::into(rhs)
    }
}

impl IntoIterator for SignalSet {
    type Item = Signal;
    type IntoIter = SignalSetIter;

    fn into_iter(self) -> Self::IntoIter {
        SignalSetIter(self.0.get())
    }
}

impl fmt::Debug for SignalSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.into_iter().fmt(f)
    }
}

/// Iterator implementation for [`SignalSet`].
///
/// # Notes
///
/// The order in which the signals are iterated over is undefined.
pub struct SignalSetIter(u8);

impl Iterator for SignalSetIter {
    type Item = Signal;

    fn next(&mut self) -> Option<Self::Item> {
        let n = self.0.trailing_zeros();
        match n {
            0 => Some(Signal::Interrupt),
            1 => Some(Signal::Quit),
            2 => Some(Signal::Terminate),
            _ => None,
        }
        .map(|signal| {
            // Remove the signal from the set.
            self.0 &= !(1 << n);
            signal
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let size = self.len();
        (size, Some(size))
    }

    fn count(self) -> usize {
        self.len()
    }
}

impl ExactSizeIterator for SignalSetIter {
    fn len(&self) -> usize {
        self.0.count_ones() as usize
    }
}

impl FusedIterator for SignalSetIter {}

impl fmt::Debug for SignalSetIter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut set = SignalSetIter(self.0);
        if set.len() == 0 {
            f.write_str("(empty)")
        } else {
            let first = set.next().unwrap();
            first.fmt(f)?;
            for signal in set {
                f.write_str("|")?;
                signal.fmt(f)?;
            }
            Ok(())
        }
    }
}

/// Signal returned by [`Signals`].
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Signal {
    /// Interrupt signal.
    ///
    /// This signal is received by the process when its controlling terminal
    /// wishes to interrupt the process. This signal will for example be send
    /// when Ctrl+C is pressed in most terminals.
    ///
    /// Corresponds to POSIX signal `SIGINT`.
    Interrupt,
    /// Termination request signal.
    ///
    /// This signal received when the process is requested to terminate. This
    /// allows the process to perform nice termination, releasing resources and
    /// saving state if appropriate. This signal will be send when using the
    /// `kill` command for example.
    ///
    /// Corresponds to POSIX signal `SIGTERM`.
    Terminate,
    /// Terminal quit signal.
    ///
    /// This signal is received when the process is requested to quit and
    /// perform a core dump.
    ///
    /// Corresponds to POSIX signal `SIGQUIT`.
    Quit,
}

impl BitOr for Signal {
    type Output = SignalSet;

    fn bitor(self, rhs: Self) -> SignalSet {
        Into::<SignalSet>::into(self) | rhs
    }
}

impl BitOr<SignalSet> for Signal {
    type Output = SignalSet;

    fn bitor(self, rhs: SignalSet) -> SignalSet {
        rhs | self
    }
}

use std::io;

use mio::{Events, Interests, Poll, Token};
use mio_signals::{Signal, SignalSet, Signals};

const SIGNAL: Token = Token(10);

fn main() -> io::Result<()> {
    // Create our `Poll` instance and events.
    let mut poll = Poll::new()?;
    let mut events = Events::with_capacity(8);

    // Create the `Signals` instance and register all possible signals.
    let mut signals = Signals::new(SignalSet::all())?;
    poll.registry()
        .register(&signals, SIGNAL, Interests::READABLE)?;

    loop {
        // Poll for events.
        poll.poll(&mut events, None)?;

        // Now send the process a signal, e.g. by pressing `CTL+C` in a shell,
        // or calling `kill` on it.

        // Process each event.
        for event in events.iter() {
            match event.token() {
                SIGNAL => {
                    // Receive the sent signal.
                    match signals.receive()? {
                        Some(Signal::Interrupt) => println!("Got interrupt signal"),
                        Some(Signal::Terminate) => {
                            println!("Got terminate signal");
                            return Ok(());
                        }
                        Some(Signal::Quit) => println!("Got quit signal"),
                        _ => println!("Got unknown signal event: {:?}", event),
                    }
                }
                _ => println!("Got unknown event: {:?}", event),
            }
        }
    }
}

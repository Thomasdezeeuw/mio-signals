use std::io;

use mio::{Events, Interest, Poll, Token};
use mio_signals::{Signal, SignalSet, Signals};

const SIGNAL: Token = Token(10);

fn main() -> io::Result<()> {
    // Create our `Poll` instance and events.
    let mut poll = Poll::new()?;
    let mut events = Events::with_capacity(8);

    // Create the `Signals` instance and register all possible signals.
    let mut signals = Signals::new(SignalSet::all())?;
    poll.registry()
        .register(&mut signals, SIGNAL, Interest::READABLE)?;

    loop {
        // Poll for events.
        match poll.poll(&mut events, None) {
            Ok(()) => {}
            // Polling can be interrupted.
            Err(ref err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }

        // Now send the process a signal, e.g. by pressing `CTL+C` in a shell,
        // or calling `kill` on it.

        // Process each event.
        for event in events.iter() {
            match event.token() {
                // Because we're using edge triggers (default in Mio) we need to
                // keep calling `receive` until it returns `Ok(None)`.
                SIGNAL => loop {
                    // Receive the sent signal.
                    match signals.receive().unwrap() {
                        Some(Signal::Interrupt) => println!("Got interrupt signal"),
                        Some(Signal::Quit) => println!("Got quit signal"),
                        Some(Signal::Terminate) => {
                            println!("Got terminate signal");
                            return Ok(());
                        }
                        None => break, // No more signals.
                    }
                },
                _ => println!("Got unknown event: {:?}", event),
            }
        }
    }
}

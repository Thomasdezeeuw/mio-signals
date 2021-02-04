use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;
use std::{io, process, thread};

use mio::{Events, Interest, Poll, Token};
use mio_signals::{send_signal, Signal, SignalSet, Signals};

const SIGNAL: Token = Token(10);
const TIMEOUT: Duration = Duration::from_secs(1);

fn main() -> io::Result<()> {
    let mut poll = Poll::new()?;
    let mut events = Events::with_capacity(8);

    let mut signals = Signals::new(SignalSet::all())?;
    poll.registry()
        .register(&mut signals, SIGNAL, Interest::READABLE)?;

    let handles = (0..5)
        .map(|_| {
            let (sender, receiver) = channel();
            let handle = thread::spawn(move || wait_for_msg(receiver));
            (sender, handle)
        })
        .collect::<Vec<_>>();

    // Send ourselves a signal.
    send_signal(process::id(), Signal::Interrupt)?;

    poll.poll(&mut events, Some(TIMEOUT))?;

    for event in events.iter() {
        match event.token() {
            SIGNAL => loop {
                match signals.receive()? {
                    Some(Signal::Interrupt) => {
                        for (sender, handle) in handles {
                            sender.send(()).unwrap();
                            handle.join().unwrap();
                        }
                        return Ok(());
                    }
                    Some(signal) => println!("Unexpected signal: {:?}", signal),
                    None => break, // No more signals.
                }
            },
            _ => println!("Got unknown event: {:?}", event),
        }
    }

    panic!("failed to get signal event");
}

fn wait_for_msg(receiver: Receiver<()>) {
    let _ = receiver.recv().unwrap();
}

use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};
use std::{io, process, thread};

use mio::{Events, Interest, Poll, Token};
use mio_signals::{send_signal, Signal, SignalSet, Signals};

const SIGNAL: Token = Token(10);
const TIMEOUT: Duration = Duration::from_secs(1);

fn main() -> io::Result<()> {
    let start = Instant::now();
    println!("\nrunning 1 test");

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

                        println!("test multi_threaded ... ok\n");
                        println!("test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in {:?}\n", start.elapsed());
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

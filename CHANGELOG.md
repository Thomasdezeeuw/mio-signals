# Changelog

## v0.2.0

* Updated to Mio v0.8.

## v0.1.5

* Add `Signal::User1` (`SIGUSR1`) and `Signal::User2` (`SIGUSR2`).

## v0.1.4

* Document correct usage in multithreaded process.
* Document use of `pthread_sigmask(3)` instead of `sigprocmask(2)`.

## v0.1.3

* Replace `sigprocmask` with `pthread_sigmask`.
* Add license file.

## v0.1.2

* Added `send_signal`: function to send a signal to a process.
* Use Mio v0.7 proper (not alpha-1).

## v0.1.1

* Make the Debug implementation of SignalSet and SignalSetIter more useful.

## v0.1.0

Initial release.

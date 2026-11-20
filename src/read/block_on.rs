//! A minimal executor for tests and synchronous callers.
//!
//! [`RangeReader`](crate::read::RangeReader) is asynchronous because its real
//! implementations are: HTTP range requests in a browser, object reads in a
//! worker. Tests and the command line tool run against sources that are
//! already resolved, so they need somewhere to drive the future to completion
//! without pulling in a runtime.
//!
//! This is not a general executor. It spins rather than parking, so it is only
//! appropriate for futures that never yield pending on external work.

use std::future::Future;
use std::task::{Context, Poll, Waker};

/// Drives `future` to completion on the current thread.
///
/// # Panics
///
/// Does not panic, but will spin forever on a future that waits on external
/// work, since it never parks and is never woken.
pub fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = Box::pin(future);
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    loop {
        if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
            return value;
        }
        std::hint::spin_loop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_a_ready_future() {
        assert_eq!(block_on(async { 1 + 1 }), 2);
    }

    #[test]
    fn runs_nested_awaits() {
        async fn inner() -> u32 {
            7
        }
        assert_eq!(block_on(async { inner().await + 1 }), 8);
    }
}

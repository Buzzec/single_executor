use crate::{EnsureSend, SleepFuture, SleepFutureRunner, SleepMessage};
use concurrency_traits::queue::TimeoutQueue;
use concurrency_traits::{TimeFunctions, TryThreadSpawner};
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use core::time::Duration;
use simple_futures::race_future::{RaceFuture, ShouldFinish};

/// A future that can timeout.
#[derive(Debug)]
pub struct TimeoutFuture<F>(RaceFuture<F, SleepFuture>);
impl<F> TimeoutFuture<F> {
    /// Creates a new timeout future from a given future.
    pub fn new<Q, CS>(future: F, sleep_runner: &SleepFutureRunner<Q, CS>, timeout: Duration) -> Self
    where
        Q: 'static + TimeoutQueue<Item = SleepMessage<CS>> + Send + Sync,
        CS: TimeFunctions + TryThreadSpawner<()>,
    {
        Self(RaceFuture::new(future, sleep_runner.sleep_for(timeout)))
    }

    /// Creates a new timeout future from a future function that takes an atomic bool that can be used to tell the future to stop when it times out.
    pub fn with_stop<Q, CS>(
        future_func: impl FnOnce(ShouldFinish) -> F,
        sleep_runner: &SleepFutureRunner<Q, CS>,
        timeout: Duration,
    ) -> Self
    where
        Q: 'static + TimeoutQueue<Item = SleepMessage<CS>> + Send + Sync,
        CS: TimeFunctions + TryThreadSpawner<()>,
    {
        Self(RaceFuture::should_finish_ok(
            future_func,
            sleep_runner.sleep_for(timeout),
        ))
    }
}
impl<F> Future for TimeoutFuture<F>
where
    F: Future,
{
    type Output = Result<F::Output, ()>;

    #[inline]
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Safety: Safe because mut ref will not move underlying value
        unsafe { Pin::new_unchecked(&mut self.get_unchecked_mut().0) }.poll(cx)
    }
}
impl<F> EnsureSend for TimeoutFuture<F> where F: Send {}

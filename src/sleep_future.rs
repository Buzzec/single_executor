use crate::{EnsureSend, EnsureSync};
use alloc::collections::BinaryHeap;
use alloc::sync::Arc;
use alloc::sync::Weak;
use concurrency_traits::queue::TimeoutQueue;
use concurrency_traits::{ThreadSpawner, TimeFunctions, TryThreadSpawner};
use core::cmp::{Ordering, Reverse};
use core::future::Future;
use core::marker::PhantomData;
use core::time::Duration;
use futures::future::AbortHandle;
use simple_futures::complete_future::{CompleteFuture, CompleteFutureHandle};

/// Runs asynchronous sleep functions by launching a separate handler thread.
/// Each new instance of this spawns a thread.
#[derive(Debug)]
pub struct SleepFutureRunner<Q, CS>
where
    CS: TryThreadSpawner<()>,
{
    inner: Arc<SleepFutureRunnerInner<Q>>,
    phantom_cs: PhantomData<fn() -> CS>,
}
impl<Q, CS> SleepFutureRunner<Q, CS>
where
    Q: 'static + TimeoutQueue<Item = SleepMessage<CS>> + Send + Sync,
    CS: TryThreadSpawner<()> + TimeFunctions,
{
    /// Tries to create a new [`SleepFutureRunner`] by launching a thread.
    pub fn try_new(queue: Q) -> Result<Self, CS::SpawnError> {
        let inner = Arc::new(SleepFutureRunnerInner { queue });
        let inner_clone = Arc::downgrade(&inner);
        CS::try_spawn(move || Self::thread_function(inner_clone))?;
        Ok(Self {
            inner,
            phantom_cs: Default::default(),
        })
    }

    /// Create a new [`SleepFutureRunner`] by launching a thread. Requires
    /// thread spawning to be infallible.
    pub fn new(queue: Q) -> Self
    where
        CS: ThreadSpawner<()>,
    {
        Self::try_new(queue).unwrap()
    }

    fn thread_function(inner: Weak<SleepFutureRunnerInner<Q>>) {
        let mut times: BinaryHeap<Reverse<SleepMessage<CS>>> = BinaryHeap::new();
        while let Some(inner) = inner.upgrade() {
            match times.pop() {
                None => {
                    if let Some(received) = inner.queue.pop_timeout(Duration::from_secs(1)) {
                        times.push(Reverse(received));
                    }
                }
                Some(Reverse(message)) => {
                    let current_time = CS::current_time();
                    match message.sleep_until <= current_time {
                        true => {
                            match &message.future {
                                SleepMessageFuture::CompleteFuture(complete) => if let Some(true) = complete.complete() {
                                    panic!("Future was already finished!");
                                }
                                SleepMessageFuture::AbortHandle(abort) => abort.abort(),
                            }
                        }
                        false => {
                            match inner.queue.pop_timeout(message.sleep_until - current_time) {
                                None => {
                                    if message.sleep_until <= CS::current_time() {
                                        match &message.future {
                                            SleepMessageFuture::CompleteFuture(complete) => if let Some(true) = complete.complete() {
                                                panic!("Future was already finished!");
                                            }
                                            SleepMessageFuture::AbortHandle(abort) => abort.abort(),
                                        }
                                    } else {
                                        times.push(Reverse(message));
                                    }
                                }
                                Some(received) => {
                                    times.push(Reverse(message));
                                    times.push(Reverse(received));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Creates a future that sleeps for a given duration from this call.
    pub fn sleep_for(&self, time: Duration) -> impl Future<Output = ()> + '_ {
        let end = CS::current_time() + time;
        self.sleep_until(end)
    }

    /// Creates a future that sleeps until a given
    /// [`CS::InstantType`](TimeFunctions::InstantType).
    pub async fn sleep_until(&self, instant: CS::InstantType) {
        if instant < CS::current_time() {
        } else {
            let complete_future = CompleteFuture::new();
            assert!(
                self.inner
                    .queue
                    .try_push(SleepMessage {
                        sleep_until: instant,
                        future: SleepMessageFuture::CompleteFuture(complete_future.get_handle()),
                    })
                    .is_ok(),
                "Could not push to sleep queue"
            );
            complete_future.await
        }
    }

    /// Runs a given future with a timeout.
    pub fn timeout_for<'a, T>(
        &'a self,
        future: impl Future<Output = T> + 'a,
        time: Duration,
    ) -> impl Future<Output = Result<T, ()>> + 'a
    where
        T: 'a,
    {
        let end = CS::current_time() + time;
        self.timeout_until(future, end)
    }

    /// Runs a given future with a timeout.
    pub async fn timeout_until<T>(
        &self,
        future: impl Future<Output = T>,
        instant: CS::InstantType,
    ) -> Result<T, ()> {
        if instant < CS::current_time() {
            Err(())
        } else {
            let (future, abort) = futures::future::abortable(future);
            assert!(
                self.inner
                    .queue
                    .try_push(SleepMessage {
                        sleep_until: instant,
                        future: SleepMessageFuture::AbortHandle(abort),
                    })
                    .is_ok(),
                "Could not push to sleep queue"
            );
            future.await.map_err(|_|())
        }
    }
}
#[derive(Debug)]
struct SleepFutureRunnerInner<Q> {
    queue: Q,
}
impl<Q, CS> EnsureSend for SleepFutureRunner<Q, CS>
where
    CS: TryThreadSpawner<()>,
    Q: Send + Sync,
{
}
impl<Q, CS> EnsureSync for SleepFutureRunner<Q, CS>
where
    CS: TryThreadSpawner<()>,
    Q: Send + Sync,
{
}

/// Internal message that the queue in [`SleepFutureRunner`] contains.
#[non_exhaustive]
#[derive(Debug)]
pub struct SleepMessage<CS>
where
    CS: TimeFunctions,
{
    sleep_until: CS::InstantType,
    future: SleepMessageFuture,
}
impl<CS> PartialEq for SleepMessage<CS>
where
    CS: TimeFunctions,
{
    fn eq(&self, other: &Self) -> bool {
        self.sleep_until.eq(&other.sleep_until)
    }
}
impl<CS> Eq for SleepMessage<CS> where CS: TimeFunctions {}
impl<CS> PartialOrd for SleepMessage<CS>
where
    CS: TimeFunctions,
{
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.sleep_until.partial_cmp(&other.sleep_until)
    }

    fn lt(&self, other: &Self) -> bool {
        self.sleep_until.lt(&other.sleep_until)
    }

    fn le(&self, other: &Self) -> bool {
        self.sleep_until.le(&other.sleep_until)
    }

    fn gt(&self, other: &Self) -> bool {
        self.sleep_until.gt(&other.sleep_until)
    }

    fn ge(&self, other: &Self) -> bool {
        self.sleep_until.ge(&other.sleep_until)
    }
}
impl<CS> Ord for SleepMessage<CS>
where
    CS: TimeFunctions,
{
    fn cmp(&self, other: &Self) -> Ordering {
        self.sleep_until.cmp(&other.sleep_until)
    }
}

/// Internal future type that [`SleepMessage`] contains.
#[non_exhaustive]
#[derive(Debug)]
pub enum SleepMessageFuture {
    /// A complete future.
    CompleteFuture(CompleteFutureHandle),
    /// An abort handle for a foreign future.
    AbortHandle(AbortHandle),
}

#[cfg(feature = "std")]
#[cfg(test)]
mod test {
    use crate::SleepFutureRunner;
    use alloc::sync::Arc;
    use alloc::task::Wake;
    use concurrency_traits::queue::ParkQueue;
    use concurrency_traits::StdThreadFunctions;
    use std::future::Future;
    use std::task::Context;
    use std::thread::{current, park, Thread};
    use std::time::{Duration, Instant};

    struct SimpleWaker(Thread);
    impl Wake for SimpleWaker {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    impl Default for SimpleWaker {
        fn default() -> Self {
            SimpleWaker(current())
        }
    }

    #[test]
    fn function_test() {
        let runner = SleepFutureRunner::<_, StdThreadFunctions>::new(ParkQueue::<
            _,
            StdThreadFunctions,
        >::default());
        let waker = Arc::new(SimpleWaker::default());
        let before = Instant::now();
        let mut future = Box::pin(runner.sleep_until(before + Duration::from_secs(1)));
        while future
            .as_mut()
            .poll(&mut Context::from_waker(&waker.clone().into()))
            .is_pending()
        {
            park();
        }
        let after = Instant::now();
        assert!(
            (after - before) > Duration::from_secs(1),
            "After: {:?}, Before: {:?}",
            after,
            before
        );
        assert!((after - before) - Duration::from_secs(1) < Duration::from_millis(10));

        let before = Instant::now();
        let mut future = Box::pin(runner.sleep_for(Duration::from_secs(1)));
        while future
            .as_mut()
            .poll(&mut Context::from_waker(&waker.clone().into()))
            .is_pending()
        {
            park();
        }
        let after = Instant::now();
        assert!((after - before) > Duration::from_secs(1));
        assert!((after - before) - Duration::from_secs(1) < Duration::from_millis(10));
    }
}

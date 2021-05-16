use crate::EnsureSend;
use alloc::sync::Arc;
use alloc::sync::Weak;
use alloc::vec::Vec;
use concurrency_traits::queue::TimeoutQueue;
use concurrency_traits::{ThreadSpawner, TimeFunctions, TryThreadSpawner};
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use core::time::Duration;
use simple_futures::complete_future::{CompleteFuture, CompleteFutureHandle};

/// Runs asynchronous sleep functions by launching a separate handler thread. Each new instance of this spawns a thread.
#[derive(Debug)]
pub struct SleepFutureRunner<Q, CS>
where
    CS: TryThreadSpawner<()>,
{
    inner: Arc<SleepFutureRunnerInner<Q>>,
    thread_handle: CS::ThreadHandle,
}
impl<'a, Q, CS> SleepFutureRunner<Q, CS>
where
    Q: 'static + TimeoutQueue<Item = SleepMessage<CS>> + Send + Sync,
    CS: TryThreadSpawner<()> + TimeFunctions,
{
    /// Tries to create a new [`SleepFutureRunner`] by launching a thread.
    pub fn try_new(queue: Q) -> Result<Self, CS::SpawnError> {
        let inner = Arc::new(SleepFutureRunnerInner { queue });
        let inner_clone = Arc::downgrade(&inner);
        Ok(Self {
            inner,
            thread_handle: CS::try_spawn(move || Self::thread_function(inner_clone))?,
        })
    }

    /// Create a new [`SleepFutureRunner`] by launching a thread. Requires thread spawning to be infallible.
    pub fn new(queue: Q) -> Self
    where
        CS: ThreadSpawner<()>,
    {
        Self::try_new(queue).unwrap()
    }

    fn thread_function(inner: Weak<SleepFutureRunnerInner<Q>>) {
        let mut times: Vec<Option<SleepMessage<CS>>> = Vec::new();
        let mut first_free = None;
        'MainLoop: while let Some(inner) = inner.upgrade() {
            let received = match times
                .iter()
                .enumerate()
                .filter_map(|(index, message)| message.as_ref().map(|message| (index, message)))
                .min_by(|message1, message2| message1.1.sleep_until.cmp(&message2.1.sleep_until))
            {
                None => inner.queue.pop(),
                Some((index, message)) => {
                    if message.sleep_until <= CS::current_time() {
                        if let Some(true) = times[index].take().unwrap().future.complete() {
                            panic!("Future was already finished!");
                        }
                        if index < first_free.unwrap_or(usize::MAX) {
                            first_free = Some(index);
                        }
                        continue 'MainLoop;
                    } else {
                        let current_time = CS::current_time();
                        let pop_result = if message.sleep_until <= current_time {
                            inner.queue.try_pop()
                        } else {
                            inner.queue.pop_timeout(message.sleep_until - current_time)
                        };
                        match pop_result {
                            None => {
                                if message.sleep_until <= CS::current_time() {
                                    if let Some(true) =
                                        times[index].take().unwrap().future.complete()
                                    {
                                        panic!("Future was already finished!");
                                    }
                                    if index < first_free.unwrap_or(usize::MAX) {
                                        first_free = Some(index);
                                    }
                                }
                                continue 'MainLoop;
                            }
                            Some(received) => received,
                        }
                    }
                }
            };

            if received.sleep_until <= CS::current_time() {
                if let Some(true) = received.future.complete() {
                    panic!("Future was already finished!");
                }
            } else {
                match first_free {
                    None => times.push(Some(received)),
                    Some(first_free_index) => {
                        times[first_free_index] = Some(received);
                        first_free = times
                            .iter()
                            .enumerate()
                            .find(|(_, message)| message.is_none())
                            .map(|(index, _)| index)
                    }
                }
            }
        }
    }

    /// Creates a future that sleeps for a given duration from this call.
    pub fn sleep_for(&self, time: Duration) -> SleepFuture {
        let complete_future = CompleteFuture::new();
        assert!(
            self.inner
                .queue
                .try_push(SleepMessage {
                    sleep_until: CS::current_time() + time,
                    future: complete_future.get_handle(),
                })
                .is_ok(),
            "Could not push to queue!"
        );
        SleepFuture(complete_future)
    }

    /// Creates a future that sleeps until a given [`CS::InstantType`](TimeFunctions::InstantType).
    pub fn sleep_until(&self, instant: CS::InstantType) -> SleepFuture {
        let complete_future = CompleteFuture::new();
        if instant < CS::current_time() {
            assert!(!complete_future.complete());
        } else {
            assert!(
                self.inner
                    .queue
                    .try_push(SleepMessage {
                        sleep_until: instant,
                        future: complete_future.get_handle(),
                    })
                    .is_ok(),
                "Could not push to queue"
            );
        }
        SleepFuture(complete_future)
    }

    /// Gets a shared reference to the [`CS::ThreadHandle`](TryThreadSpawner::ThreadHandle) that is running the sleep operations.
    pub fn thread_handle(&self) -> &CS::ThreadHandle {
        &self.thread_handle
    }

    /// Converts this runner into a . If this is the last runner for this thread then the thread will stop soon.
    pub fn into_thread_handle(self) -> CS::ThreadHandle {
        self.thread_handle
    }
}
#[derive(Debug)]
struct SleepFutureRunnerInner<Q> {
    queue: Q,
}

/// The future given by [`SleepFutureRunner`].
#[derive(Debug)]
pub struct SleepFuture(CompleteFuture);
impl Future for SleepFuture {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx)
    }
}
impl EnsureSend for SleepFuture {}

/// Internal message that the queue in [`SleepFutureRunner`] contains.
#[derive(Debug)]
pub struct SleepMessage<CS>
where
    CS: TimeFunctions,
{
    sleep_until: CS::InstantType,
    future: CompleteFutureHandle,
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

use simple_futures::complete_future::{CompleteFuture, CompleteFutureHandle};
use core::time::Duration;
use concurrency_traits::{TryThreadSpawner, TimeFunctions, ThreadSpawner};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::future::Future;
use concurrency_traits::queue::TimeoutQueue;

#[derive(Debug)]
pub struct SleepFutureRunner<Q, CS> where CS: TryThreadSpawner<()>{
    inner: Arc<SleepFutureRunnerInner<Q>>,
    thread_handle: CS::ThreadHandle,
}
impl<'a, Q, CS> SleepFutureRunner<Q, CS> where Q: 'static + TimeoutQueue<Item=SleepMessage<CS>> + Send + Sync, CS: TryThreadSpawner<()> + TimeFunctions{
    pub fn try_new(queue: Q) -> Result<Self, CS::SpawnError>{
        let inner = Arc::new(SleepFutureRunnerInner{ queue });
        let inner_clone = inner.clone();
        Ok(Self{
            inner,
            thread_handle: CS::try_spawn(move||Self::thread_function(inner_clone))?,
        })
    }

    pub fn new(queue: Q) -> Self where CS: ThreadSpawner<()>{
        Self::try_new(queue).unwrap()
    }

    fn thread_function(inner: Arc<SleepFutureRunnerInner<Q>>){
        let mut times: Vec<Option<SleepMessage<CS>>> = Vec::new();
        let mut first_free = None;
        'MainLoop: loop{
            let received = match times.iter()
                .enumerate()
                .filter_map(|(index, message)|message.as_ref().map(|message|(index, message)))
                .min_by(|message1, message2| message1.1.sleep_until.cmp(&message2.1.sleep_until)){
                None => inner.queue.pop(),
                Some((index, message)) => {
                    if message.sleep_until <= CS::current_time(){
                        if let Some(true) = times[index].take().unwrap().future.complete(){
                            panic!("Future was already finished!");
                        }
                        if index < first_free.unwrap_or(usize::MAX){
                            first_free = Some(index);
                        }
                        continue 'MainLoop;
                    }
                    else{
                        match inner.queue.pop_timeout(message.sleep_until - CS::current_time()){
                            None => {
                                if message.sleep_until <= CS::current_time(){
                                    if let Some(true) = times[index].take().unwrap().future.complete(){
                                        panic!("Future was already finished!");
                                    }
                                    if index < first_free.unwrap_or(usize::MAX){
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

            if received.sleep_until <= CS::current_time(){
                if let Some(true) = received.future.complete(){
                    panic!("Future was already finished!");
                }
            }
            else{
                match first_free {
                    None => times.push(Some(received)),
                    Some(first_free_index) => {
                        times[first_free_index] = Some(received);
                        first_free = times
                            .iter()
                            .enumerate()
                            .find(|(_, message)|message.is_none())
                            .map(|(index, _)|index)
                    }
                }
            }
        }
    }

    pub fn sleep_for(&self, time: Duration) -> impl Future<Output=()>{
        let complete_future = CompleteFuture::new();
        assert!(self.inner.queue.try_push(SleepMessage{
            sleep_until: CS::current_time() + time,
            future: complete_future.get_handle(),
        }).is_ok(), "Could not push to queue!");
        complete_future
    }

    pub fn sleep_until(&self, instant: CS::InstantType) -> impl Future<Output=()>{
        let complete_future = CompleteFuture::new();
        if instant < CS::current_time(){
            assert!(!complete_future.complete());
        }
        else{
            assert!(self.inner.queue.try_push(SleepMessage{
                sleep_until: instant,
                future: complete_future.get_handle(),
            }).is_ok(), "Could not push to queue");
        }
        complete_future
    }

    pub fn thread_handle(&self) -> &CS::ThreadHandle{
        &self.thread_handle
    }

    pub fn into_thread_handle(self) -> CS::ThreadHandle{
        self.thread_handle
    }
}
#[derive(Debug)]
struct SleepFutureRunnerInner<Q>{
    queue: Q,
}

#[derive(Debug)]
pub struct SleepMessage<CS> where CS: TimeFunctions{
    sleep_until: CS::InstantType,
    future: CompleteFutureHandle,
}

#[cfg(test)]
mod test{
    use crate::SleepFutureRunner;
    use concurrency_traits::queue::ParkQueue;
    use concurrency_traits::StdThreadFunctions;
    use alloc::task::Wake;
    use alloc::sync::Arc;
    use std::thread::{Thread, current, park};
    use std::time::{Instant, Duration};
    use std::task::Context;
    use std::future::Future;

    struct SimpleWaker(Thread);
    impl Wake for SimpleWaker {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    impl Default for SimpleWaker{
        fn default() -> Self {
            SimpleWaker(current())
        }
    }

    #[test]
    fn function_test(){
        let runner = SleepFutureRunner::<_, StdThreadFunctions>::new(ParkQueue::<_, StdThreadFunctions>::default());
        let waker = Arc::new(SimpleWaker::default());
        let before = Instant::now();
        let mut future = Box::pin(runner.sleep_until(before + Duration::from_secs(1)));
        while future.as_mut().poll(&mut Context::from_waker(&waker.clone().into())).is_pending() {
            park();
        }
        let after = Instant::now();
        assert!((after - before) > Duration::from_secs(1), "After: {:?}, Before: {:?}", after, before);
        assert!((after - before) - Duration::from_secs(1) < Duration::from_millis(10));

        let before = Instant::now();
        let mut future = Box::pin(runner.sleep_for(Duration::from_secs(1)));
        while future.as_mut().poll(&mut Context::from_waker(&waker.clone().into())).is_pending() {
            park();
        }
        let after = Instant::now();
        assert!((after - before) > Duration::from_secs(1));
        assert!((after - before) - Duration::from_secs(1) < Duration::from_millis(10));
    }
}

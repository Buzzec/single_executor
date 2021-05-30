#![cfg_attr(not(any(test, feature = "std")), no_std)]
#![warn(missing_debug_implementations, missing_docs, unused_import_braces)]

//! A single-threaded async executor.

extern crate alloc;

mod async_task;
mod atomic_state;
mod multi_complete_future;
mod polling_future;
mod sleep_future;
mod timeout_future;

pub use async_task::*;
pub use atomic_state::*;
pub use multi_complete_future::*;
pub use polling_future::*;
pub use sleep_future::*;
pub use timeout_future::*;

use alloc::boxed::Box;
use alloc::sync::{Arc, Weak};
use concurrency_traits::queue::{TimeoutQueue, TryQueue};
use concurrency_traits::{ConcurrentSystem, ThreadSpawner, TryThreadSpawner};
use core::fmt;
use core::fmt::Debug;
use core::future::Future;
use core::marker::PhantomData;
use core::ops::Deref;
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::{RawWaker, RawWakerVTable, Waker};
use core::time::Duration;
use simple_futures::value_future::ValueFuture;

trait EnsureSend: Send {}
trait EnsureSync: Sync {}

/// Returns a future that will contain the result of `function`.
/// Function will be called in another thread as to not block the main
/// thread. Fallible version of [`spawn_blocking`].
pub fn try_spawn_blocking<F, T, CS>(
    function: F,
) -> Result<(impl Future<Output = T> + 'static + Send, CS::ThreadHandle), CS::SpawnError>
where
    F: FnOnce() -> T + Send + 'static,
    T: 'static + Send,
    CS: TryThreadSpawner<()>,
{
    let future = ValueFuture::new();
    let handle = future.get_handle();
    let task_return = CS::try_spawn(move || {
        if let Some(val) = handle.assign(function()) {
            val.unwrap_or_else(|_| panic!("Could not assign from blocking!"))
        }
    })?;
    Ok((future, task_return))
}

/// Returns a future that will contain the result of `function`.
/// Function will be called in another thread as to not block the main
/// thread. Infallible version of [`try_spawn_blocking`].
pub fn spawn_blocking<F, T, CS>(
    function: F,
) -> (impl Future<Output = T> + 'static + Send, CS::ThreadHandle)
where
    F: FnOnce() -> T + Send + 'static,
    T: 'static + Send,
    CS: ThreadSpawner<()> + 'static,
{
    try_spawn_blocking::<_, _, CS>(function).unwrap()
}

/// An async executor that uses std functions.
#[cfg(feature = "std")]
pub type AsyncExecutorStd<Q> = AsyncExecutor<Q, concurrency_traits::StdThreadFunctions>;

/// An asynchronous executor that can be used to run multiple async tasks.
/// All user code runs in a single thread becasue the v5 is single threaded.
/// Blocked tasks will stop running and wait to be unblocked while also not
/// blocking the main thread.
///
/// # Panics
/// This will panic if [`Q::try_push`](concurrency_traits::queue::TryQueue::try_push) ever fails.
///
/// # Example
/// ```
/// # #[cfg(feature = "std")]
/// # {
/// use concurrency_traits::StdThreadFunctions;
/// use concurrency_traits::queue::ParkQueueStd;
/// use single_executor::{SleepFutureRunner, spawn_blocking, AsyncExecutorStd};
/// use std::rc::Rc;
/// use std::sync::atomic::{AtomicBool, Ordering};
/// use std::sync::Arc;
/// use std::thread;
/// use std::thread::sleep;
/// use std::time::Duration;
///
/// let executor = AsyncExecutorStd::new(ParkQueueStd::default());
/// let sleep_runner = Rc::new(SleepFutureRunner::new(ParkQueueStd::default()));
///
/// let sleep_runner_clone = sleep_runner.clone();
/// let loop_function = move ||{
///     let sleep_runner_clone = sleep_runner_clone.clone();
///     async move {
///         // dummy code but shows how you can await
///         sleep_runner_clone.sleep_for(Duration::from_millis(100)).await;
///         // Do stuff
///     }
/// };
/// executor.submit_loop(
///     loop_function,
///     Duration::from_millis(10),
///     sleep_runner
/// );
///
/// /// Dummy function
/// async fn get_something_from_io(){}
/// executor.submit(get_something_from_io());
///
/// /// Dummy blocking function
/// fn block_for_a_while() -> usize{
///     std::thread::sleep(Duration::from_millis(100));
///     100
/// }
/// executor.submit(async {
///     assert_eq!(spawn_blocking::<_, _, StdThreadFunctions>(block_for_a_while).0.await, 100);
/// });
///
/// // Nothing runs until run is called on the executor
/// let stop = Arc::new(AtomicBool::new(false));
/// let stop_clone = stop.clone();
/// thread::spawn(move || {
///     sleep(Duration::from_secs(1));
///     stop_clone.store(true, Ordering::Relaxed);
/// });
/// executor.run(stop); // Keeps running until stop is set to true
/// # }
/// ```
///
/// MAKE SURE NONE OF YOUR SUBMISSIONS BLOCK OR YOUR WHOLE PROGRAM WILL COME
/// CRASHING DOWN!
#[derive(Debug)]
pub struct AsyncExecutor<Q, CS> {
    task_queue: Arc<Q>,
    phantom_cs: PhantomData<fn() -> CS>,
    /// Block send and sync
    phantom_send_sync: PhantomData<*const ()>,
}
impl<Q, CS> AsyncExecutor<Q, CS>
where
    Q: 'static + TimeoutQueue<Item = AsyncTask> + Send + Sync,
    CS: ConcurrentSystem<()>,
{
    /// Creates a new executor from a given queue
    pub fn new(task_queue: Q) -> Self {
        Self {
            task_queue: Arc::new(task_queue),
            phantom_cs: Default::default(),
            phantom_send_sync: Default::default(),
        }
    }

    /// Creates a new executor from `Q`'s [`From<T>`](std::convert::From)
    /// implementation. Usually used for converting from an initial size.
    pub fn queue_from<T>(from: T) -> Self
    where
        Q: From<T>,
    {
        Self::new(Q::from(from))
    }

    /// Gets a handle to the executor through which tasks can be submitted.
    pub fn handle(&self) -> ExecutorHandle<Q> {
        ExecutorHandle {
            queue: Arc::downgrade(&self.task_queue),
        }
    }

    /// Gets a handle to the executor through which tasks can be submitted. This handle may not be sent across threads but may submit [`!Send`](Send) futures.
    pub fn local_handle(&self) -> LocalExecutorHandle<Q> {
        LocalExecutorHandle {
            queue: Arc::downgrade(&self.task_queue),
            phantom_send_sync: Default::default(),
        }
    }

    /// Adds a new future to the executor.
    /// This can be called from within a future.
    /// If this is a long running future (like a loop) then make use of sleep or
    /// use `spawn_loop` instead.
    pub fn submit(&self, future: impl Future<Output = ()> + 'static) {
        self.task_queue
            .try_push(AsyncTask::new(future))
            .expect("Queue is full when spawning!");
    }

    /// Adds a new future that will be called at a set rate.
    /// Do not do a min loop inside the future, this function handles that for
    /// you.
    pub fn submit_loop<SQ, Func, Fut>(
        &self,
        mut future_func: Func,
        delay: Duration,
        sleep_runner: impl Deref<Target = SleepFutureRunner<SQ, CS>> + 'static,
    ) where
        SQ: 'static + TimeoutQueue<Item = SleepMessage<CS>> + Send + Sync,
        Func: FnMut() -> Fut + 'static,
        Fut: Future<Output = ()>,
    {
        let future = async move {
            loop {
                let last = CS::current_time();
                future_func().await;
                sleep_runner.sleep_until(last + delay).await;
            }
        };
        self.submit(future)
    }

    /// Runs the executor, must be called or no futures will run.
    pub fn run(&self, stop: impl Deref<Target = AtomicBool>) {
        let mut _run_iters: usize = 0;
        while !stop.load(Ordering::Acquire) {
            let task = self.task_queue.pop_timeout(Duration::from_millis(10));
            if let Some(task) = task {
                let waker_data = WakerData {
                    task_queue: self.task_queue.clone(),
                    task: task.clone(),
                };
                let waker = Waker::from(waker_data);
                unsafe {
                    task.poll(&waker);
                }
            }
            _run_iters += 1;
        }
    }
}

#[derive(Clone)]
struct WakerData {
    /// Could be weak but the overhead isn't worth it to ensure dropping sooner
    task_queue: Arc<dyn TryQueue<Item = AsyncTask> + Send + Sync>,
    task: AsyncTask,
}
impl EnsureSend for WakerData {}
impl From<WakerData> for Waker {
    fn from(from: WakerData) -> Self {
        unsafe { Waker::from_raw(RawWaker::from(from)) }
    }
}
impl From<WakerData> for RawWaker {
    fn from(from: WakerData) -> Self {
        RawWaker::new(Box::into_raw(Box::new(from)) as *const (), &WAKER_VTABLE)
    }
}
static WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
    |ptr| {
        let queue: &WakerData = unsafe { &*(ptr as *const WakerData) };
        RawWaker::from(queue.clone())
    },
    |ptr| {
        let data = unsafe { Box::from_raw(ptr as *const WakerData as *mut WakerData) };
        data.task_queue.try_push(data.task).expect("Queue is full!");
    },
    |ptr| {
        let data: &WakerData = unsafe { &*(ptr as *const WakerData) };
        data.task_queue
            .try_push(data.task.clone())
            .expect("Queue is full!");
    },
    |ptr| {
        let data = unsafe { Box::from_raw(ptr as *const WakerData as *mut WakerData) };
        drop(data);
    },
);

/// A handle to an executor allowing submission of tasks.
#[derive(Debug)]
pub struct ExecutorHandle<Q> {
    queue: Weak<Q>,
}
impl<Q> ExecutorHandle<Q>
where
    Q: 'static + TimeoutQueue<Item = AsyncTask> + Send + Sync,
{
    /// Submits a task to the executor this handle came from. Will return [`Err`] if the executor was dropped.
    pub fn submit<F>(&self, future: F) -> Result<(), F>
    where
        F: Future<Output = ()> + 'static + Send,
    {
        match self.queue.upgrade() {
            None => Err(future),
            Some(queue) => {
                queue
                    .try_push(AsyncTask::new(future))
                    .expect("Queue is full!");
                Ok(())
            }
        }
    }
}
impl<Q> Clone for ExecutorHandle<Q> {
    fn clone(&self) -> Self {
        Self {
            queue: self.queue.clone(),
        }
    }
}

/// A handle to an executor allowing submission of tasks. This handle may not be sent across threads but can submit [`!Send`](Send) futures.
#[derive(Debug)]
pub struct LocalExecutorHandle<Q> {
    queue: Weak<Q>,
    /// Block send and sync
    phantom_send_sync: PhantomData<*const ()>,
}
impl<Q> LocalExecutorHandle<Q>
where
    Q: 'static + TimeoutQueue<Item = AsyncTask> + Send + Sync,
{
    /// Submits a task to the executor this handle came from. Will return [`Err`] if the executor was dropped.
    pub fn submit<F>(&self, future: F) -> Result<(), F>
    where
        F: Future<Output = ()> + 'static,
    {
        match self.queue.upgrade() {
            None => Err(future),
            Some(queue) => {
                queue
                    .try_push(AsyncTask::new(future))
                    .expect("Queue is full!");
                Ok(())
            }
        }
    }
}
impl<Q> Clone for LocalExecutorHandle<Q> {
    fn clone(&self) -> Self {
        Self {
            queue: self.queue.clone(),
            phantom_send_sync: Default::default(),
        }
    }
}

#[cfg(feature = "std")]
#[cfg(test)]
mod test {
    use crate::{AsyncExecutor, SleepFutureRunner};
    use concurrency_traits::queue::ParkQueue;
    use concurrency_traits::StdThreadFunctions;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
    use std::sync::Arc;
    use std::thread::{sleep, spawn};
    use std::time::Duration;

    #[test]
    fn slam_test() {
        let executor = AsyncExecutor::<_, StdThreadFunctions>::new(ParkQueue::<
            _,
            StdThreadFunctions,
        >::default());
        let sleep_runner = Rc::new(SleepFutureRunner::<
            ParkQueue<_, StdThreadFunctions>,
            StdThreadFunctions,
        >::new(Default::default()));
        let loop_function = |atom_count: Rc<AtomicIsize>| async move {
            atom_count.fetch_add(1, Ordering::SeqCst);
        };
        let mut atom_counts = Vec::with_capacity(100);
        for _ in 0..100 {
            let atom_count = Rc::new(AtomicIsize::new(0));
            atom_counts.push(atom_count.clone());
            executor.submit_loop(
                move || {
                    let atom_count = atom_count.clone();
                    loop_function(atom_count)
                },
                Duration::from_millis(100),
                sleep_runner.clone(),
            );
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = stop.clone();
        spawn(move || {
            sleep(Duration::from_secs(1));
            stop_clone.store(true, Ordering::Release);
        });
        executor.run(stop);
        for count in &atom_counts {
            assert!((count.load(Ordering::SeqCst) - 10).abs() < 5);
        }
    }
}

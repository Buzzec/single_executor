use alloc::sync::Weak;
use core::future::Future;
use core::marker::PhantomData;

use concurrency_traits::queue::TryQueue;

use crate::AsyncTask;
use alloc::string::String;

/// A handle to an executor allowing submission of tasks. This handle may not be
/// sent across threads but can submit [`!Send`](Send) futures.
#[derive(Debug)]
pub struct LocalExecutorHandle<Q> {
    queue: Weak<Q>,
    /// Block send and sync
    phantom_send_sync: PhantomData<*const ()>,
}
impl<Q> LocalExecutorHandle<Q>
where
    Q: 'static + TryQueue<Item = AsyncTask> + Send + Sync,
{
    pub(crate) fn from_queue(queue: Weak<Q>) -> Self {
        Self {
            queue,
            phantom_send_sync: Default::default(),
        }
    }
    /// Submits a task to the executor this handle came from. Will return
    /// [`Err`] if the executor was dropped.
    pub fn submit<F, S>(&self, future: F, task_name: S) -> Result<(), (F, S)>
    where
        F: Future<Output = ()> + 'static,
        S: Into<String>,
    {
        match self.queue.upgrade() {
            None => Err((future, task_name)),
            Some(queue) => {
                queue
                    .try_push(AsyncTask::new(future, task_name.into()))
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

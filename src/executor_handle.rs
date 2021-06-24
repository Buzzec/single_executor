use alloc::sync::Weak;
use core::future::Future;

use concurrency_traits::queue::TimeoutQueue;

use crate::AsyncTask;
use alloc::string::String;

/// A handle to an executor allowing submission of tasks.
#[derive(Debug)]
pub struct SendExecutorHandle<Q> {
    pub(crate) queue: Weak<Q>,
}
impl<Q> SendExecutorHandle<Q>
where
    Q: 'static + TimeoutQueue<Item = AsyncTask> + Send + Sync,
{
    /// Submits a task to the executor this handle came from. Will return
    /// [`Err`] if the executor was dropped.
    pub fn submit<F, S>(&self, future: F, task_name: S) -> Result<(), (F, S)>
    where
        F: Future<Output = ()> + 'static + Send,
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
impl<Q> Clone for SendExecutorHandle<Q> {
    fn clone(&self) -> Self {
        Self {
            queue: self.queue.clone(),
        }
    }
}

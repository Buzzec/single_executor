use core::pin::Pin;
use alloc::sync::Arc;
use core::cell::UnsafeCell;
use core::future::Future;
use core::fmt::{Debug, Formatter};
use crate::fmt;
use core::task::{Waker, Context, Poll};

type TaskFuture = Pin<Arc<UnsafeCell<dyn Future<Output = ()>>>>;
/// The tasks used by [`AsyncExecutor`](crate::AsyncExecutor).
pub struct AsyncTask {
    future: TaskFuture,
}
impl<'a> Debug for AsyncTask {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Task").field("future", &"omitted").finish()
    }
}
impl<'a> Clone for AsyncTask {
    fn clone(&self) -> Self {
        Self {
            future: self.future.clone(),
        }
    }

    fn clone_from(&mut self, source: &Self) {
        self.future.clone_from(&source.future);
    }
}
impl AsyncTask {
    /// Creates a new async task
    pub(crate) fn new(future: impl Future<Output = ()> + 'static) -> Self {
        Self {
            future: Arc::pin(UnsafeCell::new(future)),
        }
    }

    /// # Safety
    /// Must be called only on executor thread
    pub(crate) unsafe fn poll(&self, waker: &Waker) -> Poll<()> {
        // Safety: Can be created because the arc ensures this future won't move and this will only be called on a single thread.
        Pin::new_unchecked(&mut *self.future.get()).poll(&mut Context::from_waker(&waker))
    }
}
// Safety: As long as nothing !Send is accessed outside the executor thread this
// is okay.
unsafe impl Send for AsyncTask {}

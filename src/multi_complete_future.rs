use alloc::collections::VecDeque;
use alloc::sync::{Arc, Weak};
use concurrency_traits::mutex::{Mutex, SpinLock};
use concurrency_traits::ThreadFunctions;
use core::future::Future;
use core::pin::Pin;
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::{Context, Poll, Waker};

/// A version of [`CompleteFuture`](simple_futures::complete_future::CompleteFuture) that can be cloned to have multiple futures pointing to the same "complete-ness".
#[derive(Debug)]
pub struct MultiCompleteFuture<CS> {
    inner: Arc<MultiCompleteFutureInner<CS>>,
}
impl<CS> MultiCompleteFuture<CS> {
    /// Gets a handle to this future that does not prevent dropping the inner.
    pub fn get_handle(&self) -> MultiCompleteFutureHandle<CS> {
        MultiCompleteFutureHandle {
            inner: Arc::downgrade(&self.inner),
        }
    }

    /// Returns true if future was already finished.
    #[inline]
    pub fn complete(&self) -> bool
    where
        CS: ThreadFunctions,
    {
        self.inner.complete()
    }
}
impl<CS> Future for MultiCompleteFuture<CS>
where
    CS: ThreadFunctions,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.inner.complete.load(Ordering::Acquire) {
            true => Poll::Ready(()),
            false => {
                let mut guard = self.inner.waker_queue.lock();
                match guard.0 {
                    true => Poll::Ready(()),
                    false => {
                        guard.1.push_back(cx.waker().clone());
                        Poll::Pending
                    }
                }
            }
        }
    }
}
impl<CS> Default for MultiCompleteFuture<CS> {
    fn default() -> Self {
        Self {
            inner: Arc::new(MultiCompleteFutureInner {
                waker_queue: SpinLock::new((false, VecDeque::new())),
                complete: AtomicBool::new(false),
            }),
        }
    }
}
impl<CS> Clone for MultiCompleteFuture<CS>{
    fn clone(&self) -> Self {
        Self{
            inner: self.inner.clone(),
        }
    }
}

#[derive(Debug)]
struct MultiCompleteFutureInner<CS> {
    waker_queue: SpinLock<(bool, VecDeque<Waker>), CS>,
    complete: AtomicBool,
}
impl<CS> MultiCompleteFutureInner<CS>
where
    CS: ThreadFunctions,
{
    /// Returns true if future was already finished.
    fn complete(&self) -> bool {
        match self.complete.swap(true, Ordering::AcqRel) {
            true => true,
            false => {
                let mut guard = self.waker_queue.lock();
                guard.0 = true;
                while let Some(waker) = guard.1.pop_front() {
                    waker.wake();
                }
                false
            }
        }
    }
}

/// A handle to a [`MultiCompleteFuture`].
#[derive(Debug)]
pub struct MultiCompleteFutureHandle<CS> {
    inner: Weak<MultiCompleteFutureInner<CS>>,
}
impl<CS> MultiCompleteFutureHandle<CS>
where
    CS: ThreadFunctions,
{
    /// Returns true if future was already finished, or [`None`] if all futures were dropped.
    pub fn complete(&self) -> Option<bool> {
        Some(self.inner.upgrade()?.complete())
    }
}

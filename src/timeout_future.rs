use core::sync::atomic::{AtomicBool, Ordering, AtomicU8};
use core::future::Future;
use crate::{SleepFutureRunner, SleepFuture, SleepMessage, EnsureSend};
use concurrency_traits::{TryThreadSpawner, TimeFunctions};
use core::time::Duration;
use core::task::{Context, Poll, Waker, RawWaker, RawWakerVTable};
use core::pin::Pin;
use concurrency_traits::queue::TimeoutQueue;
use alloc::sync::Arc;
use atomic_swapping::option::AtomicSwapOption;
use bitflags::bitflags;
use core::mem::forget;

/// A future that can timeout.
#[derive(Debug)]
pub struct TimeoutFuture<F>{
    inner: Arc<TimeoutFutureInner>,
    should_stop: Option<Arc<AtomicBool>>,
    future: F,
    timeout_future: SleepFuture,
}
impl<F> TimeoutFuture<F> {
    /// Creates a new timeout future from a given future.
    pub fn new<Q, CS>(future: F, sleep_runner: &SleepFutureRunner<Q, CS>, timeout: Duration) -> Self where Q: 'static + TimeoutQueue<Item=SleepMessage<CS>> + Send + Sync, CS: TimeFunctions + TryThreadSpawner<()> {
        Self {
            inner: Arc::new(TimeoutFutureInner {
                state: AtomicU8::new(TimeoutFutureState::MAIN.bits & TimeoutFutureState::TIMEOUT.bits),
                waker: AtomicSwapOption::from_none(),
            }),
            future,
            should_stop: None,
            timeout_future: sleep_runner.sleep_for(timeout),
        }
    }

    /// Creates a new timeout future from a future function that takes an atomic bool that can be used to tell the future to stop when it times out.
    pub fn with_stop<Q, CS>(future_func: impl FnOnce(Arc<AtomicBool>) -> F, sleep_runner: &SleepFutureRunner<Q, CS>, timeout: Duration) -> Self where Q: 'static + TimeoutQueue<Item=SleepMessage<CS>> + Send + Sync, CS: TimeFunctions + TryThreadSpawner<()> {
        let stop = Arc::new(AtomicBool::new(false));
        Self {
            inner: Arc::new(TimeoutFutureInner {
                state: AtomicU8::new(TimeoutFutureState::MAIN.bits & TimeoutFutureState::TIMEOUT.bits),
                waker: AtomicSwapOption::from_none(),
            }),
            future: future_func(stop.clone()),
            should_stop: Some(stop),
            timeout_future: sleep_runner.sleep_for(timeout),
        }
    }
}
impl<F> TimeoutFuture<F> where F: Future{
    unsafe fn poll_future(&mut self) -> Poll<F::Output>{
        Pin::new_unchecked(&mut self.future).poll(&mut Context::from_waker(&Waker::from_raw(self.inner.clone().into_future_waker())))
    }

    fn poll_timeout(&mut self) -> Poll<()>{
        Pin::new(&mut self.timeout_future).poll(&mut Context::from_waker(unsafe{ &Waker::from_raw(self.inner.clone().into_timeout_waker()) }))
    }
}
impl<F> Future for TimeoutFuture<F> where F: Future{
    type Output = Result<F::Output, ()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Safety safe to pin because mut ref never leaks and never moves value behind it
        let self_ref = unsafe { self.get_unchecked_mut() };

        let mut state = self_ref.inner.state.swap(TimeoutFutureState::POLLING.bits, Ordering::AcqRel);
        self_ref.inner.waker.swap(Some(cx.waker().clone()));
        loop {
            if state & TimeoutFutureState::MAIN.bits == TimeoutFutureState::MAIN.bits {
                if let Poll::Ready(val) = unsafe { self_ref.poll_future() } {
                    return Poll::Ready(Ok(val));
                }
            }
            if state & TimeoutFutureState::TIMEOUT.bits == TimeoutFutureState::TIMEOUT.bits && self_ref.poll_timeout().is_ready() {
                if let Some(should_stop) = &self_ref.should_stop{
                    should_stop.store(true, Ordering::Release);
                }
                return Poll::Ready(Err(()));
            }
            match self_ref.inner.state.compare_exchange(TimeoutFutureState::POLLING.bits, TimeoutFutureState::WAITING.bits, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => break,
                Err(new_state) => state = new_state,
            }
        }
        Poll::Pending
    }
}
impl<F> EnsureSend for TimeoutFuture<F> where F: Send{}

#[derive(Debug)]
struct TimeoutFutureInner{
    waker: AtomicSwapOption<Waker>,
    state: AtomicU8,
}
impl TimeoutFutureInner{
    fn into_future_waker(self: Arc<Self>) -> RawWaker{
        RawWaker::new(Arc::into_raw(self) as *const (), &FUTURE_WAKER_VTABLE)
    }

    fn into_timeout_waker(self: Arc<Self>) -> RawWaker{
        RawWaker::new(Arc::into_raw(self) as *const (), &TIMEOUT_WAKER_VTABLE)
    }
}

static FUTURE_WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
    |ptr|unsafe{ clone_raw(ptr).into_future_waker() },
    |ptr|{
        let inner = unsafe{ get_arc(ptr) };
        let old = inner.state.fetch_and(TimeoutFutureState::MAIN.bits, Ordering::AcqRel);
        if old & (TimeoutFutureState::TIMEOUT.bits | TimeoutFutureState::POLLING.bits) == 0{
            inner.waker.take().unwrap().wake();
        }
    },
    |ptr|{
        let inner = unsafe{ get_arc(ptr) };
        let old = inner.state.fetch_and(TimeoutFutureState::MAIN.bits, Ordering::AcqRel);
        if old & (TimeoutFutureState::TIMEOUT.bits | TimeoutFutureState::POLLING.bits) == 0{
            inner.waker.take().unwrap().wake();
        }
        forget(inner);
    },
    drop_raw,
);
static TIMEOUT_WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
    |ptr|unsafe{ clone_raw(ptr).into_timeout_waker() },
    |ptr|{
        let inner = unsafe{ get_arc(ptr) };
        let old = inner.state.fetch_and(TimeoutFutureState::TIMEOUT.bits, Ordering::AcqRel);
        if old & (TimeoutFutureState::MAIN.bits | TimeoutFutureState::POLLING.bits) == 0{
            inner.waker.take().unwrap().wake();
        }
    },
    |ptr|{
        let inner = unsafe{ get_arc(ptr) };
        let old = inner.state.fetch_and(TimeoutFutureState::TIMEOUT.bits, Ordering::AcqRel);
        if old & (TimeoutFutureState::MAIN.bits | TimeoutFutureState::POLLING.bits) == 0{
            inner.waker.take().unwrap().wake();
        }
        forget(inner)
    },
    drop_raw,
);

unsafe fn get_arc(ptr: *const ()) -> Arc<TimeoutFutureInner>{
    Arc::from_raw(ptr as *const TimeoutFutureInner)
}
unsafe fn clone_raw(ptr: *const ()) -> Arc<TimeoutFutureInner>{
    let inner = get_arc(ptr);
    let out = inner.clone();
    forget(inner);
    out
}
unsafe fn drop_raw(ptr: *const ()){
    drop(get_arc(ptr));
}

bitflags!{
    struct TimeoutFutureState: u8{
        const WAITING = 0b00000000;
        const TIMEOUT = 0b00000001;
        const MAIN    = 0b00000010;
        const POLLING = 0b00000100;
    }
}

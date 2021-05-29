use crate::{SleepFutureRunner, SleepMessage};
use concurrency_traits::queue::TimeoutQueue;
use concurrency_traits::{TimeFunctions, TryThreadSpawner};
use core::time::Duration;

/// Polls a function (`function`) until it returns true. Sleeps between polls for `sleep_duration`.
pub async fn polling_future<Q, CS>(
    mut function: impl FnMut() -> bool,
    sleep_duration: Duration,
    sleep_future_runner: SleepFutureRunner<Q, CS>,
) where
    CS: TryThreadSpawner<()> + TimeFunctions,
    Q: 'static + TimeoutQueue<Item = SleepMessage<CS>> + Send + Sync,
{
    while !function(){
        sleep_future_runner.sleep_for(sleep_duration).await;
    }
}

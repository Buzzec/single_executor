### single_executor v0.3.2
- Added `Default` to `MultiCompleteFuture`
- Added `polling_future` function

### single_executor v0.3.1
- Added `Clone` to executor handles

## single_executor v0.3.0
- Updated `concurrency_traits` to v0.7
- Added `MultiCompleteFuture`

## single_executor v0.2.0
- Changed `TimeoutFuture` to use `RaceFuture` from `simple_futures`

### single_executor v0.1.1
- Added Atomic States
- Added `TimeoutFuture`
- Made `SleepFutureRunner` return `SleepFuture`s rather than `impl Future<Output = ()>`. This is non-breaking because `SleepFuture` implements `Future<Output = ()>`.

# single_executor v0.1.0
- Initial Version!

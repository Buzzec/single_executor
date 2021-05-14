# single_executor v0.1.1
- Added Atomic States
- Added `TimeoutFuture`
- Made `SleepFutureRunner` return `SleepFuture`s rather than `impl Future<Output = ()>`. This is non-breaking because `SleepFuture` implements `Future<Output = ()>`.

# single_executor v0.1.0
- Initial Version!

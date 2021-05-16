use core::sync::atomic::*;

macro_rules! atomic_state {
    ($vis:vis, $name:ident, $atomic_type:ty, $raw_type:ty) => {
        /// An atomic that stores a given state in a `$raw_type`
        #[derive(Debug)]
        #[repr(transparent)]
        $vis struct $name<T>{
            state: $atomic_type,
            phantom_t: core::marker::PhantomData<fn() -> T>,
        }
        impl<T> $name<T> where $raw_type: From<T>{
            /// Analog for [`$atomic_type::new`].
            pub fn new(state: T) -> Self{
                Self{ state: <$atomic_type>::new(state.into()), phantom_t: Default::default() }
            }

            /// Analog for [`$atomic_type::store`].
            pub fn store(&self, state: T, ordering: Ordering){
                self.state.store(state.into(), ordering)
            }
        }
        impl<T> $name<T> where T: From<$raw_type>{
            /// Analog for [`$atomic_type::load`].
            pub fn load(&self, ordering: Ordering) -> T{
                self.state.load(ordering).into()
            }

            /// Analog for [`$atomic_type::into_inner`].
            pub fn into_inner(self) -> T{
                self.state.into_inner().into()
            }
        }
        impl<T> $name<T> where T: From<$raw_type>, $raw_type: From<T>{
            /// Analog for [`$atomic_type::get_mut`].
            pub fn set(&mut self, modify_func: impl FnOnce(T) -> T){
                let mut_val = self.state.get_mut();
                *mut_val = modify_func((*mut_val).into()).into();
            }

            /// Analog for [`$atomic_type::swap`].
            pub fn swap(&self, state: T, ordering: Ordering) -> T{
                self.state.swap(state.into(), ordering).into()
            }

            /// Analog for [`$atomic_type::compare_exchange`].
            pub fn compare_exchange(&self, current: T, new: T, success: Ordering, failure: Ordering) -> Result<T, T>{
                match self.state.compare_exchange(current.into(), new.into(), success, failure){
                    Ok(val) => Ok(val.into()),
                    Err(val) => Err(val.into()),
                }
            }

            /// Analog for [`$atomic_type::compare_exchange_weak`].
            pub fn compare_exchange_weak(&self, current: T, new: T, success: Ordering, failure: Ordering) -> Result<T, T>{
                match self.state.compare_exchange_weak(current.into(), new.into(), success, failure){
                    Ok(val) => Ok(val.into()),
                    Err(val) => Err(val.into()),
                }
            }

            /// Analog for [`$atomic_type::fetch_update`].
            pub fn fetch_update(&self, set_order: Ordering, fetch_order: Ordering, mut f: impl FnMut(T) -> Option<T>) -> Result<T, T>{
                match self.state.fetch_update(set_order, fetch_order, |val|f(val.into()).map(|val|val.into())){
                    Ok(val) => Ok(val.into()),
                    Err(val) => Err(val.into()),
                }
            }
        }
    };
}

atomic_state!(pub, AtomicU8State, AtomicU8, u8);
atomic_state!(pub, AtomicU16State, AtomicU16, u16);
atomic_state!(pub, AtomicU32State, AtomicU32, u32);
atomic_state!(pub, AtomicU64State, AtomicU64, u64);
atomic_state!(pub, AtomicI8State, AtomicI8, i8);
atomic_state!(pub, AtomicI16State, AtomicI16, i16);
atomic_state!(pub, AtomicI32State, AtomicI32, i32);
atomic_state!(pub, AtomicI64State, AtomicI64, i64);

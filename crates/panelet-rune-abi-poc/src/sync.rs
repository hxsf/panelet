// SPDX-License-Identifier: MPL-2.0

//! Reference counts and mutex bookkeeping for host-owned device state.
//!
//! On ESP-IDF, allocate Arc's complete control block in INTERNAL | 8BIT RAM.
//! FreeRTOS allocates its mutex queues with the same explicit capabilities.
//! Unlike the default malloc size preference, neither route falls back to PSRAM.
//! Store Mutex itself inside an internal Arc (or on an internal task stack).
//! Mutex is task-only, nonrecursive, and must never be used from an ISR.
use std::ops::Deref;

#[cfg(target_os = "espidf")]
type Shared<T> = std::sync::Arc<T, InternalAllocator>;
#[cfg(not(target_os = "espidf"))]
type Shared<T> = std::sync::Arc<T>;
#[cfg(target_os = "espidf")]
type SharedWeak<T> = std::sync::Weak<T, InternalAllocator>;
#[cfg(not(target_os = "espidf"))]
type SharedWeak<T> = std::sync::Weak<T>;

/// Sized host metadata. Rune's own VM/Unit Arcs remain their native types.
pub struct Arc<T>(Shared<T>);
pub struct Weak<T>(SharedWeak<T>);
impl<T> Arc<T> {
    pub fn new(value: T) -> Self {
        #[cfg(target_os = "espidf")]
        {
            Self(
                Shared::try_new_in(value, InternalAllocator)
                    .unwrap_or_else(|_| panic!("internal Arc allocation failed")),
            )
        }
        #[cfg(not(target_os = "espidf"))]
        {
            Self(Shared::new(value))
        }
    }
    pub fn downgrade(this: &Self) -> Weak<T> {
        Weak(Shared::downgrade(&this.0))
    }
    pub fn as_ptr(this: &Self) -> *const T {
        Shared::as_ptr(&this.0)
    }
    pub fn ptr_eq(this: &Self, other: &Self) -> bool {
        Shared::ptr_eq(&this.0, &other.0)
    }
    pub fn strong_count(this: &Self) -> usize {
        Shared::strong_count(&this.0)
    }
}
impl<T> Clone for Arc<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<T> rune::alloc::clone::TryClone for Arc<T> {
    fn try_clone(&self) -> Result<Self, rune::alloc::Error> {
        Ok(self.clone())
    }
}
impl<T> Deref for Arc<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}
impl<T: std::fmt::Debug> std::fmt::Debug for Arc<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&**self, f)
    }
}
impl<T> Weak<T> {
    pub fn upgrade(&self) -> Option<Arc<T>> {
        self.0.upgrade().map(Arc)
    }
    pub fn ptr_eq(this: &Self, other: &Self) -> bool {
        SharedWeak::ptr_eq(&this.0, &other.0)
    }
}
impl<T> Clone for Weak<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

#[cfg(target_os = "espidf")]
#[derive(Clone, Copy, Debug)]
struct InternalAllocator;
#[cfg(target_os = "espidf")]
unsafe impl std::alloc::Allocator for InternalAllocator {
    fn allocate(
        &self,
        layout: std::alloc::Layout,
    ) -> Result<std::ptr::NonNull<[u8]>, std::alloc::AllocError> {
        use std::ptr::NonNull;
        if layout.size() == 0 {
            // Aligned, non-null sentinel; zero-size blocks are never dereferenced.
            let ptr = NonNull::new(layout.align() as *mut u8).unwrap();
            return Ok(NonNull::slice_from_raw_parts(ptr, 0));
        }
        let size = layout.pad_to_align().size();
        // SAFETY: alignment is a power of two, size is its multiple. Explicit
        // capabilities prohibit the allocator's default PSRAM fallback.
        let ptr = unsafe {
            esp_idf_sys::heap_caps_aligned_alloc(
                layout.align(),
                size,
                esp_idf_sys::MALLOC_CAP_INTERNAL | esp_idf_sys::MALLOC_CAP_8BIT,
            )
        } as *mut u8;
        let ptr = NonNull::new(ptr).ok_or(std::alloc::AllocError)?;
        Ok(NonNull::slice_from_raw_parts(ptr, layout.size()))
    }
    unsafe fn deallocate(&self, ptr: std::ptr::NonNull<u8>, layout: std::alloc::Layout) {
        if layout.size() != 0 {
            // SAFETY: this allocator produced ptr and the caller relinquishes it.
            unsafe { esp_idf_sys::heap_caps_free(ptr.as_ptr().cast()) };
        }
    }
}

#[cfg(not(target_os = "espidf"))]
pub use std::sync::{Mutex, MutexGuard};

#[cfg(target_os = "espidf")]
mod target {
    use esp_idf_sys::{self as sys, QueueDefinition, QueueHandle_t};
    use std::cell::UnsafeCell;
    use std::marker::PhantomData;
    use std::ops::{Deref, DerefMut};
    use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
    use std::sync::{LockResult, PoisonError};

    // FreeRTOS queue.h macros are not present in bindgen output.
    const QUEUE_TYPE_MUTEX: u8 = 1;
    const SEND_TO_BACK: sys::BaseType_t = 0;

    pub struct Mutex<T> {
        handle: AtomicPtr<QueueDefinition>,
        poisoned: AtomicBool,
        value: UnsafeCell<T>,
    }
    // Moving a protected T between tasks requires T: Send. All access to T is
    // serialized by the nonrecursive FreeRTOS mutex, including immutable reads.
    unsafe impl<T: Send> Send for Mutex<T> {}
    unsafe impl<T: Send> Sync for Mutex<T> {}
    impl<T> Mutex<T> {
        pub const fn new(value: T) -> Self {
            Self {
                handle: AtomicPtr::new(std::ptr::null_mut()),
                poisoned: AtomicBool::new(false),
                value: UnsafeCell::new(value),
            }
        }
        fn handle(&self) -> QueueHandle_t {
            let existing = self.handle.load(Ordering::Acquire);
            if !existing.is_null() {
                return existing;
            }
            // SAFETY: queue type denotes a nonrecursive mutex. ESP-IDF's
            // FreeRTOS pvPortMalloc uses INTERNAL | 8BIT unconditionally.
            let candidate = unsafe { sys::xQueueCreateMutex(QUEUE_TYPE_MUTEX) };
            assert!(
                !candidate.is_null(),
                "internal FreeRTOS mutex allocation failed"
            );
            match self.handle.compare_exchange(
                std::ptr::null_mut(),
                candidate,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => candidate,
                Err(winner) => {
                    // SAFETY: no task saw or locked this losing candidate.
                    unsafe { sys::vQueueDelete(candidate) };
                    winner
                }
            }
        }
        pub fn lock(&self) -> LockResult<MutexGuard<'_, T>> {
            let handle = self.handle();
            // SAFETY: handle remains initialized and alive for this borrow.
            // TickType_t::MAX is portMAX_DELAY for the configured tick width.
            let taken = unsafe { sys::xQueueSemaphoreTake(handle, sys::TickType_t::MAX) };
            assert!(taken != 0, "FreeRTOS mutex lock failed");
            let guard = MutexGuard {
                mutex: self,
                handle,
                entered_panicking: std::thread::panicking(),
                not_send: PhantomData,
            };
            if self.poisoned.load(Ordering::Acquire) {
                Err(PoisonError::new(guard))
            } else {
                Ok(guard)
            }
        }
    }
    impl<T: Default> Default for Mutex<T> {
        fn default() -> Self {
            Self::new(T::default())
        }
    }
    impl<T> Drop for Mutex<T> {
        fn drop(&mut self) {
            let handle = *self.handle.get_mut();
            if !handle.is_null() {
                // SAFETY: exclusive self access precludes surviving guards or
                // waiters; the handle was installed exactly once by handle().
                unsafe { sys::vQueueDelete(handle) };
            }
        }
    }
    pub struct MutexGuard<'a, T> {
        mutex: &'a Mutex<T>,
        handle: QueueHandle_t,
        entered_panicking: bool,
        // A FreeRTOS mutex must be released by its acquiring task.
        not_send: PhantomData<*mut ()>,
    }
    unsafe impl<T: Sync> Sync for MutexGuard<'_, T> {}
    impl<T> Deref for MutexGuard<'_, T> {
        type Target = T;
        fn deref(&self) -> &T {
            // SAFETY: this guard owns the mutex until its Drop runs.
            unsafe { &*self.mutex.value.get() }
        }
    }
    impl<T> DerefMut for MutexGuard<'_, T> {
        fn deref_mut(&mut self) -> &mut T {
            // SAFETY: nonrecursive mutex and exclusive guard borrow guarantee
            // no other reference to the protected value can alias this one.
            unsafe { &mut *self.mutex.value.get() }
        }
    }
    impl<T> Drop for MutexGuard<'_, T> {
        fn drop(&mut self) {
            if !self.entered_panicking && std::thread::panicking() {
                self.mutex.poisoned.store(true, Ordering::Release);
            }
            // SAFETY: this task owns the mutex. Mutex queues hold no item data;
            // these are the arguments used by FreeRTOS xSemaphoreGive.
            let released =
                unsafe { sys::xQueueGenericSend(self.handle, std::ptr::null(), 0, SEND_TO_BACK) };
            assert!(released != 0, "FreeRTOS mutex unlock failed");
        }
    }
}
#[cfg(target_os = "espidf")]
pub use target::{Mutex, MutexGuard};

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[test]
    fn strong_and_weak_ownership_drop_payload_exactly_once() {
        struct CountDrop(std::sync::Arc<AtomicUsize>);
        impl Drop for CountDrop {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let drops = std::sync::Arc::new(AtomicUsize::new(0));
        let value = Arc::new(CountDrop(drops.clone()));
        let weak = Arc::downgrade(&value);
        let second = rune::alloc::clone::TryClone::try_clone(&value).unwrap();
        assert!(Arc::ptr_eq(&value, &second));
        assert!(Weak::ptr_eq(&weak, &weak.clone()));
        assert_eq!(Arc::strong_count(&value), 2);
        drop(value);
        assert!(weak.upgrade().is_some());
        drop(second);
        assert!(weak.upgrade().is_none());
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn cloned_metadata_serializes_thread_updates() {
        let value = Arc::new(Mutex::new(0));
        let tasks: Vec<_> = (0..4)
            .map(|_| {
                let value = value.clone();
                std::thread::spawn(move || {
                    for _ in 0..1000 {
                        *value.lock().unwrap() += 1;
                    }
                })
            })
            .collect();
        for task in tasks {
            task.join().unwrap();
        }
        assert_eq!(*value.lock().unwrap(), 4000);
    }
    #[test]
    fn mutex_can_be_initialized_const_and_relocked() {
        static VALUE: Mutex<u32> = Mutex::new(0);
        *VALUE.lock().unwrap() = 42;
        assert_eq!(*VALUE.lock().unwrap(), 42);
    }
}

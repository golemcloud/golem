//! # A store of pre-initialized values.
//!
//! Values can be checked out when needed, operated on, and will automatically
//! be returned to the pool when they go out of scope. It can be used when
//! handling values that are expensive to create. Based on the [object pool
//! pattern](http://en.wikipedia.org/wiki/Object_pool_pattern).
//!
//! Example:
//!
//! ```
//! use poule::{Pool, Dirty};
//! use std::thread;
//!
//! let mut pool = Pool::with_capacity(20);
//! pool.grow_to(10);
//!
//! let mut vec = pool.checkout(|| Dirty(Vec::with_capacity(16_384))).unwrap();
//!
//! // Do some work with the value, this can happen in another thread
//! thread::spawn(move || {
//!     for i in 0..10_000 {
//!         vec.push(i);
//!     }
//!
//!     assert_eq!(10_000, vec.len());
//! }).join();
//!
//! // The vec will have been returned to the pool by now
//! let vec = pool.checkout(|| Dirty(Vec::with_capacity(16_384))).unwrap();
//!
//! // The pool operates LIFO, so this vec will be the same value that was used
//! // in the thread above. The value will also be left as it was when it was
//! // returned to the pool, this may or may not be desirable depending on the
//! // use case.
//! assert_eq!(10_000, vec.len());
//!
//! ```
//!
//! ## Threading
//!
//! Checking out values from the pool requires a mutable reference to the pool
//! so cannot happen concurrently across threads, but returning values to the
//! pool is thread safe and lock free, so if the value being pooled is `Sync`
//! then `Checkout<T>` is `Sync` as well.
//!
//! The easiest way to have a single pool shared across many threads would be
//! to wrap `Pool` in a mutex.
pub use reset::{Dirty, Reset};
use std::cell::UnsafeCell;
use std::sync::atomic::{self, AtomicUsize, Ordering};
use std::sync::Arc;
use std::{mem, ops, ptr, usize};

mod mmap;
mod reset;

/// A pool of reusable values
pub struct Pool<T: Reset> {
    inner: Arc<UnsafeCell<PoolInner<T>>>,
}

impl<T: Reset> Pool<T> {
    /// Creates a new pool that can contain up to `maximum` entries.
    pub fn with_capacity(maximum: usize) -> Pool<T> {
        Self::with_extra(maximum, 0)
    }

    /// Creates a new pool that can contain up to `maximum` entries.
    /// each entry can have `extra` additional bytes to store data
    pub fn with_extra(maximum: usize, extra: usize) -> Pool<T> {
        let inner = PoolInner::with_capacity(maximum, extra);
        Pool {
            inner: Arc::new(UnsafeCell::new(inner)),
        }
    }

    pub fn grow_to(&mut self, count: usize) {
        self.inner_mut()
            .grow_to(count)
            .expect("could not grow pool");
    }

    /// Checkout a value from the pool. Returns `None` if the pool is currently
    /// at capacity.
    ///
    /// The value returned from the pool has not been reset and contains the
    /// state that it previously had when it was last released.
    pub fn checkout<F>(&mut self, init: F) -> Option<Checkout<T>>
    where
        F: Fn() -> T,
    {
        let entry = match self.inner_mut().checkout() {
            Some(e) => Some(e),
            None => {
                if self.inner_mut().initialize(init) {
                    self.inner_mut().checkout()
                } else {
                    None
                }
            }
        };

        entry
            .map(|ptr| Checkout {
                entry: ptr,
                inner: self.inner.clone(),
            })
            .map(|mut checkout| {
                checkout.reset();
                checkout
            })
    }

    fn inner_mut(&self) -> &mut PoolInner<T> {
        unsafe { mem::transmute(self.inner.get()) }
    }

    pub fn capacity(&self) -> usize {
        self.inner_mut().count
    }

    pub fn maximum_capacity(&self) -> usize {
        self.inner_mut().maximum
    }

    pub fn len(&self) -> usize {
        self.inner_mut().init
    }

    pub fn memory_size(&self) -> usize {
        self.inner_mut().count * self.inner_mut().entry_size
    }

    pub fn used(&self) -> usize {
        self.inner_mut().used.load(Ordering::Relaxed)
    }
}

unsafe impl<T: Send + Reset> Send for Pool<T> {}

/// A handle to a checked out value. When dropped out of scope, the value will
/// be returned to the pool.
pub struct Checkout<T> {
    entry: *mut Entry<T>,
    inner: Arc<UnsafeCell<PoolInner<T>>>,
}

impl<T> Checkout<T> {
    /// Read access to the raw bytes
    pub fn extra(&self) -> &[u8] {
        self.entry().extra()
    }

    /// Write access to the extra bytes
    pub fn extra_mut(&mut self) -> &mut [u8] {
        self.entry_mut().extra_mut()
    }

    fn entry(&self) -> &Entry<T> {
        unsafe { mem::transmute(self.entry) }
    }

    fn entry_mut(&mut self) -> &mut Entry<T> {
        unsafe { mem::transmute(self.entry) }
    }

    fn inner(&self) -> &mut PoolInner<T> {
        unsafe { mem::transmute(self.inner.get()) }
    }
}

impl<T> ops::Deref for Checkout<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.entry().data
    }
}

impl<T> ops::DerefMut for Checkout<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.entry_mut().data
    }
}

impl<T> Drop for Checkout<T> {
    fn drop(&mut self) {
        self.inner().checkin(self.entry);
    }
}

unsafe impl<T: Send> Send for Checkout<T> {}
unsafe impl<T: Sync> Sync for Checkout<T> {}

struct PoolInner<T> {
    #[allow(dead_code)]
    memory: mmap::GrowableMemoryMap, // Ownership of raw memory
    next: AtomicUsize,  // Offset to next available value
    ptr: *mut Entry<T>, // Pointer to first entry
    init: usize,        // Number of initialized entries
    count: usize,       // Total number of entries
    maximum: usize,     // maximum number of entries
    entry_size: usize,  // Byte size of each entry
    used: AtomicUsize,  // Number of elements currently checked out
}

// Max size of the pool
const MAX: usize = usize::MAX >> 1;

impl<T> PoolInner<T> {
    fn with_capacity(count: usize, mut extra: usize) -> PoolInner<T> {
        // The required alignment for the entry. The start of the entry must
        // align with this number
        let align = mem::align_of::<Entry<T>>();

        // Check that the capacity is not too large
        assert!(count < MAX, "requested pool size too big");
        assert!(
            align > 0,
            "something weird is up with the requested alignment"
        );

        let mask = align - 1;

        // If the requested extra memory does not match with the align,
        // increase it so that it does.
        if extra & mask != 0 {
            extra = (extra + align) & !mask;
        }

        // Calculate the size of each entry. Since the extra bytes are
        // immediately after the entry, just add the sizes
        let entry_size = mem::size_of::<Entry<T>>() + extra;

        // This should always be true, but let's check it anyway
        assert!(entry_size & mask == 0, "entry size is not aligned");

        // Ensure that the total memory needed is possible. It must be
        // representable by an `isize` value in order for pointer offset to
        // work.
        assert!(
            entry_size.checked_mul(count).is_some(),
            "requested pool capacity too big"
        );
        assert!(entry_size * count < MAX, "requested pool capacity too big");

        let size = count * entry_size;

        // Allocate the memory
        let memory = mmap::GrowableMemoryMap::new(size).expect("could not generate memory map");
        let ptr = memory.ptr();

        PoolInner {
            memory,
            next: AtomicUsize::new(0),
            ptr: ptr as *mut Entry<T>,
            init: 0,
            count: 0,
            maximum: count,
            entry_size,
            used: AtomicUsize::new(0),
        }
    }

    fn grow_to(&mut self, count: usize) -> Result<(), &'static str> {
        if count > self.maximum {
            return Err("cannot grow larger than the maximum number of entries");
        }

        let size = count * self.entry_size;
        self.memory.grow_to(size)?;
        self.count = count;

        Ok(())
    }

    fn initialize<F>(&mut self, initializer: F) -> bool
        where F: Fn() -> T {

        if self.init < self.count {
            unsafe {
                ptr::write(
                    self.entry_mut(self.init),
                    Entry {
                        data: initializer(),
                        next: self.init + 1,
                        extra: self.entry_size - mem::size_of::<Entry<T>>(),
                    },
                    );
            }
            self.init += 1;

            true
        } else {
            false
        }
    }

    fn checkout(&mut self) -> Option<*mut Entry<T>> {
        let mut idx = self.next.load(Ordering::Acquire);

        loop {
            debug_assert!(idx <= self.count, "invalid index: {}", idx);

            if idx == self.init {
                // The pool is depleted
                return None;
            }

            let nxt = self.entry_mut(idx).next;

            debug_assert!(nxt <= self.init, "invalid next index: {}", idx);

            match self.next.compare_exchange(idx, nxt, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => break,
                Err(res) => {
                    // Re-acquire the memory before trying again
                    atomic::fence(Ordering::Acquire);
                    idx = res;
                }
            }
        }

        self.used.fetch_add(1, Ordering::Relaxed);
        Some(self.entry_mut(idx) as *mut Entry<T>)
    }

    fn checkin(&self, ptr: *mut Entry<T>) {
        let idx;
        let entry: &mut Entry<T>;

        unsafe {
            // Figure out the index
            idx = ((ptr as usize) - (self.ptr as usize)) / self.entry_size;
            entry = mem::transmute(ptr);
        }

        debug_assert!(idx < self.count, "invalid index; idx={}", idx);

        let mut nxt = self.next.load(Ordering::Relaxed);

        loop {
            // Update the entry's next pointer
            entry.next = nxt;

            match self.next.compare_exchange(nxt, idx, Ordering::Release, Ordering::Relaxed) {
                Ok(_) => break,
                Err(actual) => nxt = actual,
            }
        }
        self.used.fetch_sub(1, Ordering::Relaxed);
    }

    fn entry(&self, idx: usize) -> &Entry<T> {
        unsafe {
            debug_assert!(idx < self.count, "invalid index");
            let ptr = (self.ptr as usize + idx * self.entry_size as usize) as *mut Entry<T>;
            mem::transmute(ptr)
        }
    }

    #[allow(mutable_transmutes)]
    fn entry_mut(&mut self, idx: usize) -> &mut Entry<T> {
        unsafe { mem::transmute(self.entry(idx)) }
    }
}

impl<T> Drop for PoolInner<T> {
    fn drop(&mut self) {
        for i in 0..self.init {
            unsafe {
                let _ = ptr::read(self.entry(i));
            }
        }
    }
}

struct Entry<T> {
    data: T,      // Keep first
    next: usize,  // Index of next available entry
    extra: usize, // Number of extra byts available
}

impl<T> Entry<T> {
    fn extra(&self) -> &[u8] {
        use std::slice;

        unsafe {
            let ptr: *const u8 = mem::transmute(self);
            let ptr = ptr.offset(mem::size_of::<Entry<T>>() as isize);

            slice::from_raw_parts(ptr, self.extra)
        }
    }

    #[allow(mutable_transmutes)]
    fn extra_mut(&mut self) -> &mut [u8] {
        unsafe { mem::transmute(self.extra()) }
    }
}

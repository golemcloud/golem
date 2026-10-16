//! Counts the heap the shell holds, for its memory budget (`brush_core::memory`).
//!
//! Every allocation, Rust's and wasi-libc's own (the lists the component model hands it, which it
//! frees itself), goes through libc's `malloc` family, so the build wraps those (`--wrap`, see
//! `build.rs`) and each wrapper counts the bytes the allocator gives or takes back, as
//! `malloc_usable_size` reports them. Calling through `__real_*` makes a build without the `--wrap`
//! flags fail to link instead of counting nothing.
use std::ffi::{c_int, c_void};

unsafe extern "C" {
    fn __real_malloc(size: usize) -> *mut c_void;
    fn __real_calloc(count: usize, size: usize) -> *mut c_void;
    fn __real_realloc(ptr: *mut c_void, size: usize) -> *mut c_void;
    fn __real_aligned_alloc(alignment: usize, size: usize) -> *mut c_void;
    fn __real_posix_memalign(out: *mut *mut c_void, alignment: usize, size: usize) -> c_int;
    fn __real_free(ptr: *mut c_void);
    fn malloc_usable_size(ptr: *mut c_void) -> usize;
}

/// The bytes the allocator holds for `ptr`, a live allocation or null.
fn held(ptr: *mut c_void) -> usize {
    if ptr.is_null() {
        0
    } else {
        // SAFETY: `ptr` is a live allocation of this allocator.
        unsafe { malloc_usable_size(ptr) }
    }
}

/// # Safety
/// libc's contract for `malloc`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __wrap_malloc(size: usize) -> *mut c_void {
    // SAFETY: libc's contract, forwarded.
    let ptr = unsafe { __real_malloc(size) };
    brush_core::memory::allocated(held(ptr));
    ptr
}

/// # Safety
/// libc's contract for `calloc`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __wrap_calloc(count: usize, size: usize) -> *mut c_void {
    // SAFETY: libc's contract, forwarded.
    let ptr = unsafe { __real_calloc(count, size) };
    brush_core::memory::allocated(held(ptr));
    ptr
}

/// # Safety
/// libc's contract for `realloc`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __wrap_realloc(ptr: *mut c_void, size: usize) -> *mut c_void {
    let before = held(ptr);
    // SAFETY: libc's contract, forwarded.
    let moved = unsafe { __real_realloc(ptr, size) };
    // A failed reallocation leaves the old one as it was.
    if !moved.is_null() || size == 0 {
        brush_core::memory::released(before);
        brush_core::memory::allocated(held(moved));
    }
    moved
}

/// # Safety
/// libc's contract for `aligned_alloc`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __wrap_aligned_alloc(alignment: usize, size: usize) -> *mut c_void {
    // SAFETY: libc's contract, forwarded.
    let ptr = unsafe { __real_aligned_alloc(alignment, size) };
    brush_core::memory::allocated(held(ptr));
    ptr
}

/// # Safety
/// libc's contract for `posix_memalign`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __wrap_posix_memalign(
    out: *mut *mut c_void,
    alignment: usize,
    size: usize,
) -> c_int {
    // SAFETY: libc's contract, forwarded.
    let status = unsafe { __real_posix_memalign(out, alignment, size) };
    if status == 0 {
        // SAFETY: on success `out` holds the new allocation.
        brush_core::memory::allocated(held(unsafe { *out }));
    }
    status
}

/// # Safety
/// libc's contract for `free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __wrap_free(ptr: *mut c_void) {
    brush_core::memory::released(held(ptr));
    // SAFETY: libc's contract, forwarded.
    unsafe { __real_free(ptr) };
}

#[cfg(test)]
mod tests {
    use brush_core::memory::in_use;

    #[test]
    fn what_rust_allocates_is_counted_until_it_is_freed() {
        let before = in_use();
        let block = std::hint::black_box(vec![0_u8; 1 << 20]);
        let held = in_use();
        drop(block);
        assert!(held >= before + (1 << 20));
        assert!(in_use() < held);
    }

    #[test]
    fn what_the_c_library_allocates_and_frees_is_counted() {
        let before = in_use();
        // SAFETY: one allocation, grown, then freed once.
        let (allocated, grown) = unsafe {
            let ptr = libc::malloc(1000);
            let allocated = in_use();
            let ptr = libc::realloc(ptr, 1 << 20);
            let grown = in_use();
            libc::free(ptr);
            (allocated, grown)
        };
        assert!(allocated >= before + 1000);
        assert!(grown >= before + (1 << 20));
        assert_eq!(in_use(), before);
    }
}

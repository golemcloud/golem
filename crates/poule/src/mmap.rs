use std::{
    ops::{Deref, DerefMut},
    slice,
};

#[cfg(unix)]
use libc::{
    mmap, mprotect, munmap, MAP_ANON, MAP_FAILED, MAP_PRIVATE, PROT_NONE, PROT_READ, PROT_WRITE,
};

#[cfg(windows)]
use windows_sys::Win32::System::Memory::{
    VirtualAlloc, VirtualFree, VirtualProtect,
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE,
    PAGE_NOACCESS, PAGE_READWRITE,
};

/// Memory map backend for the pool
///
/// This acts like a buffer that we can grow without changing its address
/// (it would not be usable for a memory pool) and without copying the data
/// to a new buffer.
///
/// it creates a memory map with a fixed capacity and the PROT_NONE option,
/// making it a very large guard page. It does not consume physical memory,
/// but reserves that part of the address space for future usage.
///
/// Whenever we want to increase the available memory, a larger part of the
/// memory map is marked as readable and writable, telling the kernel to
/// map physical memory.
pub struct GrowableMemoryMap {
    ptr: *mut u8,
    capacity: usize,
    size: usize,
}

#[cfg(windows)]
impl GrowableMemoryMap {
    pub fn new(capacity: usize) -> Result<Self, &'static str> {
        let capacity = page_size(capacity);

        let ptr = unsafe {
            VirtualAlloc(
                std::ptr::null_mut(),
                capacity,
                MEM_RESERVE,
                PAGE_NOACCESS,
            )
        };

        if ptr.is_null() {
            return Err("could not map memory");
        }

        Ok(GrowableMemoryMap {
            ptr: ptr as *mut u8,
            capacity,
            size: 0,
        })
    }

    pub fn grow_to(&mut self, size: usize) -> Result<(), &'static str> {
        let size = page_size(size);

        if size <= self.size {
            return Ok(());
        }

        if size > self.capacity {
            return Err("new size cannot be larger than max capacity");
        }

        unsafe {
            // Commit the memory and change protection
            if VirtualAlloc(
                self.ptr as _,
                size,
                MEM_COMMIT,
                PAGE_READWRITE,
            ).is_null() {
                return Err("could not commit memory");
            }

            let mut old_protect = 0;
            if VirtualProtect(
                self.ptr as _,
                size,
                PAGE_READWRITE,
                &mut old_protect,
            ) == 0 {
                return Err("could not change permissions on memory");
            }
        }

        self.size = size;
        Ok(())
    }
}

#[cfg(unix)]
impl GrowableMemoryMap {
    pub fn new(capacity: usize) -> Result<Self, &'static str> {
        let capacity = page_size(capacity);

        let ptr = unsafe {
            mmap(
                std::ptr::null_mut(),
                capacity,
                PROT_NONE,
                MAP_ANON | MAP_PRIVATE,
                -1,
                0,
            )
        };

        if ptr == MAP_FAILED {
            return Err("could not map memory");
        }

        Ok(GrowableMemoryMap {
            ptr: ptr as *mut u8,
            capacity,
            size: 0,
        })
    }

    pub fn grow_to(&mut self, size: usize) -> Result<(), &'static str> {
        let size = page_size(size);

        if size <= self.size {
            return Ok(());
        }

        if size > self.capacity {
            return Err("new size cannot be larger than max capacity");
        }

        if unsafe { mprotect(self.ptr as _, size, PROT_READ | PROT_WRITE) != 0 } {
            return Err("could not change permissions on memory");
        }
        self.size = size;

        Ok(())
    }
}

// Common implementations for both platforms
impl GrowableMemoryMap {
    pub fn ptr(&self) -> *mut u8 {
        self.ptr
    }
}

impl Deref for GrowableMemoryMap {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(self.ptr, self.size) }
    }
}

impl DerefMut for GrowableMemoryMap {
    fn deref_mut(&mut self) -> &mut [u8] {
        unsafe { slice::from_raw_parts_mut(self.ptr, self.size) }
    }
}

#[cfg(unix)]
impl Drop for GrowableMemoryMap {
    fn drop(&mut self) {
        unsafe {
            if munmap(self.ptr as _, self.capacity) != 0 {
                println!("could not unmap");
            }
        }
    }
}

#[cfg(windows)]
impl Drop for GrowableMemoryMap {
    fn drop(&mut self) {
        unsafe {
            if VirtualFree(self.ptr as _, 0, MEM_RELEASE) == 0 {
                println!("could not unmap");
            }
        }
    }
}

pub fn page_size(data_len: usize) -> usize {
    let page_size = if cfg!(windows) { 0x1000 } else { 0x1000 };
    let count = data_len / page_size;
    let rem = data_len % page_size;

    if rem == 0 {
        data_len
    } else {
        (count + 1) * page_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    pub fn guard_page() {
        //let (ptr, size) = alloc_with_guard();
        let mut map = GrowableMemoryMap::new(32768).unwrap();
        println!("new map size: {} ({:?})", (*map).len(), &*map);
        assert_eq!((*map).len(), 0);

        map.grow_to(4096).unwrap();
        println!("new map size: {} ({:?})", (*map).len(), &*map);
        assert_eq!((*map).len(), 4096);
        let sl = &mut *map;
        sl[0] = 1;
        println!("map content: {:?}", &(*map)[..32]);
        map.grow_to(8192).unwrap();
        println!("new map size: {} ({:?})", (*map).len(), &*map);
        println!("map content: {:?}", &(*map)[..32]);
        assert_eq!((*map).len(), 8192);
        let sl2 = &mut *map;
        assert_eq!(sl2[0], 1);
    }
}

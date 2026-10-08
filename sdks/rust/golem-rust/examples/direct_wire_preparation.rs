//! Opt-in direct-WIT preparation measurement. Build the adjacent standalone
//! `direct-wire-preparation/Cargo.toml` with `--release --target wasm32-wasip1`
//! and run the WASI command guest to observe allocations after optimization.

use golem_schema::schema::wit::direct::{IntoWire, decode, encode_async};
use std::alloc::{GlobalAlloc, Layout, System};
use std::future::Future;
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Waker};
use std::time::Instant;

struct CountingAllocator;
const COUNT_ALLOCATIONS: bool = option_env!("GOL720_COUNT_ALLOCATIONS").is_some();
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNT_ALLOCATIONS {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if COUNT_ALLOCATIONS {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(size, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn ready<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("resource-free preparation must not suspend"),
    }
}

fn measure(name: &str, size: usize, iterations: usize, mut operation: impl FnMut()) {
    for _ in 0..10 {
        operation();
    }
    for sample in 0..7 {
        ALLOCATIONS.store(0, Ordering::Relaxed);
        BYTES.store(0, Ordering::Relaxed);
        let start = Instant::now();
        for _ in 0..iterations {
            operation();
        }
        let nanos = start.elapsed().as_nanos();
        let allocations = ALLOCATIONS.load(Ordering::Relaxed);
        let bytes = BYTES.load(Ordering::Relaxed);
        println!("{name},{size},{iterations},{sample},{nanos},{allocations},{bytes}");
    }
}

fn main() {
    eprintln!("allocation_instrumentation={COUNT_ALLOCATIONS}");
    println!("operation,elements,iterations,sample,nanoseconds,allocations,allocated_bytes");
    for size in [100, 10_000] {
        let values: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
        let nested: Vec<Vec<u8>> = values.chunks(10).map(<[u8]>::to_vec).collect();
        assert_eq!(
            decode::<Vec<u8>>(ready(encode_async(&values)).unwrap()).unwrap(),
            values
        );
        assert_eq!(
            decode::<Vec<Vec<u8>>>(ready(encode_async(&nested)).unwrap()).unwrap(),
            nested
        );
        measure("prepare_vec_u8", size, 1_000, || {
            ready(black_box(&values).prepare_wire()).unwrap();
        });
        measure("prepare_nested_vec_u8", size, 1_000, || {
            ready(black_box(&nested).prepare_wire()).unwrap();
        });
        measure("encode_vec_u8", size, 100, || {
            let tree = ready(encode_async(black_box(&values))).unwrap();
            assert_eq!(tree.value_nodes.len(), size + 1);
            black_box(tree);
        });
    }
}

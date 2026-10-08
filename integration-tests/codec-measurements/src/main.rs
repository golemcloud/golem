use golem_common::data_value;
use golem_common::schema::{ExternalSchemaValue, ExternalTypedSchemaValue, TypedSchemaValue};
use serde::Serialize;
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::time::Instant;

struct CountingAllocator;
static ENABLED: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static ALLOCATED_BYTES: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn allocated(size: usize) {
    let live = LIVE.fetch_add(size, Relaxed) + size;
    if ENABLED.load(Relaxed) {
        ALLOCATIONS.fetch_add(1, Relaxed);
        ALLOCATED_BYTES.fetch_add(size, Relaxed);
        PEAK.fetch_max(live, Relaxed);
    }
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            allocated(layout.size());
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            allocated(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Relaxed);
        unsafe { System.dealloc(ptr, layout) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let result = unsafe { System.realloc(ptr, layout, size) };
        if !result.is_null() {
            LIVE.fetch_sub(layout.size(), Relaxed);
            allocated(size);
        }
        result
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[derive(Serialize)]
struct Sample {
    elapsed_ns: u128,
    allocations: usize,
    allocated_bytes: usize,
    incremental_peak_live_bytes: usize,
}

#[derive(Serialize)]
struct Measurement {
    stage: &'static str,
    length: usize,
    payload_bytes: usize,
    median_ns: u128,
    min_ns: u128,
    max_ns: u128,
    samples: Vec<Sample>,
}

fn measure<T>(
    stage: &'static str,
    length: usize,
    payload_bytes: usize,
    mut run: impl FnMut() -> T,
) {
    for _ in 0..3 {
        drop(black_box(run()));
    }
    let mut samples = Vec::with_capacity(9);
    for _ in 0..9 {
        ALLOCATIONS.store(0, Relaxed);
        ALLOCATED_BYTES.store(0, Relaxed);
        let live = LIVE.load(Relaxed);
        PEAK.store(live, Relaxed);
        ENABLED.store(true, Relaxed);
        let start = Instant::now();
        drop(black_box(run()));
        let elapsed_ns = start.elapsed().as_nanos();
        ENABLED.store(false, Relaxed);
        samples.push(Sample {
            elapsed_ns,
            allocations: ALLOCATIONS.load(Relaxed),
            allocated_bytes: ALLOCATED_BYTES.load(Relaxed),
            incremental_peak_live_bytes: PEAK.load(Relaxed).saturating_sub(live),
        });
    }
    let mut times: Vec<_> = samples.iter().map(|sample| sample.elapsed_ns).collect();
    times.sort_unstable();
    let result = Measurement {
        stage,
        length,
        payload_bytes,
        median_ns: times[4],
        min_ns: times[0],
        max_ns: times[8],
        samples,
    };
    println!("{}", serde_json::to_string(&result).unwrap());
}

fn main() {
    // Asymmetric bytes catch corruption that an all-zero list would hide.
    for length in [100, 10_000] {
        let bytes: Vec<u8> = (0..length).map(|i| (i % 251) as u8).collect();
        let typed: TypedSchemaValue = data_value!(bytes.clone());
        let external = ExternalSchemaValue::try_from(typed.value().clone()).unwrap();
        let external_typed = ExternalTypedSchemaValue::try_from(typed.clone()).unwrap();
        let encoded = serde_json::to_vec(&external).unwrap();
        let encoded_typed = serde_json::to_vec(&external_typed).unwrap();
        let decoded: ExternalSchemaValue = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, external);
        let decoded_typed: ExternalTypedSchemaValue =
            serde_json::from_slice(&encoded_typed).unwrap();
        assert_eq!(decoded_typed, external_typed);
        assert_eq!(
            serde_json::to_value(&external).unwrap(),
            serde_json::from_slice::<serde_json::Value>(&encoded).unwrap()
        );

        measure("client-construct-and-check", length, encoded.len(), || {
            let typed = data_value!(bytes.clone());
            ExternalSchemaValue::try_from(typed.into_parts().1).unwrap()
        });
        measure("external-encode-bytes", length, encoded.len(), || {
            serde_json::to_vec(&external).unwrap()
        });
        measure("external-encode-dom", length, encoded.len(), || {
            serde_json::to_value(&external).unwrap()
        });
        measure("external-decode-bytes", length, encoded.len(), || {
            serde_json::from_slice::<ExternalSchemaValue>(&encoded).unwrap()
        });
        measure(
            "external-decode-dom-including-parse",
            length,
            encoded.len(),
            || {
                let dom = serde_json::from_slice::<serde_json::Value>(&encoded).unwrap();
                serde_json::from_value::<ExternalSchemaValue>(dom).unwrap()
            },
        );
        measure(
            "typed-external-encode-bytes",
            length,
            encoded_typed.len(),
            || serde_json::to_vec(&external_typed).unwrap(),
        );
        measure(
            "typed-external-decode-bytes",
            length,
            encoded_typed.len(),
            || serde_json::from_slice::<ExternalTypedSchemaValue>(&encoded_typed).unwrap(),
        );
        measure("harness-only-clone-value", length, encoded.len(), || {
            typed.value().clone()
        });
        // The clone supplies ownership for repeated samples; report it separately.
        measure(
            "harness-only-clone-envelope-and-consume",
            length,
            encoded.len(),
            || typed.clone().into_parts().1,
        );
    }
}

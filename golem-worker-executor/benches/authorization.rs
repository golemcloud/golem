// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use golem_common::model::card::owner::{
    AgentOwnerPattern, EmptyOwnerPattern, EnvironmentOwnerPattern,
};
use golem_common::model::card::{
    ClassPermissionTarget, EffectiveSurface, FilesystemClass, FilesystemPathPattern,
    FilesystemResourcePattern, FilesystemVerb, GrantSurface, KvClass, KvResourcePattern, KvVerb,
    NetworkResourcePattern, NetworkVerb, PermissionTarget, PortPattern, ResourcePattern,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

struct CountingAllocator;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn network_target(host: impl Into<String>, port: u16) -> PermissionTarget {
    PermissionTarget::Network(ClassPermissionTarget {
        verb: Some(NetworkVerb::Connect),
        owner: EmptyOwnerPattern,
        resource: NetworkResourcePattern::host_port(host, PortPattern::single(port)),
    })
}

fn kv_target(store: &str, key: &str) -> PermissionTarget {
    PermissionTarget::Kv(ClassPermissionTarget::<KvClass> {
        verb: Some(KvVerb::Read),
        owner: EnvironmentOwnerPattern::parse("acme/shop/prod").unwrap(),
        resource: KvResourcePattern::parse_resource(&format!("{store}.{key}")).unwrap(),
    })
}

fn filesystem_target(path: &str) -> PermissionTarget {
    PermissionTarget::Filesystem(ClassPermissionTarget::<FilesystemClass> {
        verb: Some(FilesystemVerb::Read),
        owner: AgentOwnerPattern::parse("acme/shop/prod/cart/agent").unwrap(),
        resource: FilesystemResourcePattern::Path(FilesystemPathPattern::parse(path).unwrap()),
    })
}

fn surface(grants: usize, requested: &PermissionTarget, allows: bool) -> EffectiveSurface {
    let mut positive = (0..grants.saturating_sub(1))
        .map(|index| network_target(format!("unmatched-{index}.example.com"), 443))
        .collect::<Vec<_>>();
    positive.push(if allows {
        requested.clone()
    } else {
        network_target("also-unmatched.example.com", 443)
    });
    EffectiveSurface {
        source_card_ids: Vec::new(),
        lower: vec![GrantSurface {
            positive,
            negative: Vec::new(),
        }],
        upper: Vec::new(),
    }
}

fn exact_surface(targets: Vec<PermissionTarget>) -> EffectiveSurface {
    EffectiveSurface {
        source_card_ids: Vec::new(),
        lower: vec![GrantSurface {
            positive: targets,
            negative: Vec::new(),
        }],
        upper: Vec::new(),
    }
}

// This benchmark-only harness measures the boundary work around matching. It is not an
// implementation of the executor's wallet synchronization machinery.
struct BoundaryOverheadHarness {
    processed_generation: u64,
}

impl BoundaryOverheadHarness {
    fn drain_adopt_and_match(
        &mut self,
        published_generation: u64,
        pending_events: u64,
        snapshot: &EffectiveSurface,
        target: &PermissionTarget,
    ) -> bool {
        assert!(published_generation > self.processed_generation);
        assert_eq!(
            published_generation - self.processed_generation,
            pending_events
        );
        for generation in (self.processed_generation + 1)..=published_generation {
            black_box(generation);
        }
        let result = snapshot.authorize(target).unwrap();
        self.processed_generation = published_generation;
        result
    }
}

fn percentile(samples: &[Duration], percentile: usize) -> Duration {
    samples[(samples.len() - 1) * percentile / 100]
}

fn record_distribution<F>(label: &str, mut operation: F, expect_zero_allocations: bool)
where
    F: FnMut() -> bool,
{
    let mut samples = Vec::with_capacity(20_000);
    for _ in 0..samples.capacity() {
        let start = Instant::now();
        black_box(operation());
        samples.push(start.elapsed());
    }
    samples.sort_unstable();

    ALLOCATIONS.store(0, Ordering::Relaxed);
    black_box(operation());
    let allocations = ALLOCATIONS.load(Ordering::Relaxed);
    if expect_zero_allocations {
        assert_eq!(
            allocations, 0,
            "this stable matching case must not allocate"
        );
    }

    eprintln!(
        "authorization/{label}: p50={:?}, p95={:?}, allocations={allocations}",
        percentile(&samples, 50),
        percentile(&samples, 95),
    );
}

fn bench_authorization(c: &mut Criterion) {
    c.bench_function("authorization/no_enforcement_baseline", |b| {
        b.iter(|| black_box(true))
    });

    let tcp_target = network_target("db.example.com", 5432);
    for (size, grants) in [("small", 8), ("medium", 64), ("large", 512)] {
        let allow = surface(grants, &tcp_target, true);
        let deny = surface(grants, &tcp_target, false);
        c.bench_function(
            &format!("authorization/stable_allow/{size}_{grants}_grants"),
            |b| b.iter(|| black_box(allow.authorize(black_box(&tcp_target)).unwrap())),
        );
        c.bench_function(
            &format!("authorization/stable_deny/{size}_{grants}_grants"),
            |b| b.iter(|| black_box(deny.authorize(black_box(&tcp_target)).unwrap())),
        );
        if grants == 64 {
            record_distribution(
                "stable_tcp_allow/medium",
                || allow.authorize(&tcp_target).unwrap(),
                true,
            );
            record_distribution(
                "stable_tcp_deny/medium",
                || deny.authorize(&tcp_target).unwrap(),
                true,
            );
        }
    }

    let kv_single = kv_target("sessions", "user-1");
    let kv_batch = (0..32)
        .map(|index| kv_target("sessions", &format!("user-{index}")))
        .collect::<Vec<_>>();
    let kv_surface = exact_surface(kv_batch.clone());
    c.bench_function("authorization/kv/single_key", |b| {
        b.iter(|| black_box(kv_surface.authorize(black_box(&kv_single)).unwrap()))
    });
    c.bench_function("authorization/kv/32_key_batch", |b| {
        b.iter(|| {
            black_box(
                kv_batch
                    .iter()
                    .all(|target| kv_surface.authorize(black_box(target)).unwrap()),
            )
        })
    });

    let fs_open = filesystem_target("/data/report.json");
    let fs_stream = filesystem_target("/data/archive/chunk-0001");
    let fs_surface = exact_surface(vec![fs_open.clone(), fs_stream.clone()]);
    for (label, target) in [("open", &fs_open), ("stream_admission", &fs_stream)] {
        c.bench_function(&format!("authorization/filesystem/{label}"), |b| {
            b.iter(|| black_box(fs_surface.authorize(black_box(target)).unwrap()))
        });
    }

    let http_target = network_target("api.example.com", 443);
    let dispatch_surface = exact_surface(vec![tcp_target.clone(), http_target.clone()]);
    for (label, target) in [("tcp", &tcp_target), ("http", &http_target)] {
        c.bench_function(&format!("authorization/dispatch/{label}"), |b| {
            b.iter(|| black_box(dispatch_surface.authorize(black_box(target)).unwrap()))
        });
    }

    record_distribution(
        "stable_filesystem_open",
        || fs_surface.authorize(&fs_open).unwrap(),
        true,
    );
    record_distribution(
        "stable_kv_single_key",
        || kv_surface.authorize(&kv_single).unwrap(),
        false,
    );

    let refresh_snapshot = surface(64, &tcp_target, true);
    c.bench_function(
        "authorization/boundary_overhead/one_generation_refresh",
        |b| {
            let mut published_generation = 0;
            let mut harness = BoundaryOverheadHarness {
                processed_generation: 0,
            };
            b.iter(|| {
                published_generation += 1;
                black_box(harness.drain_adopt_and_match(
                    published_generation,
                    1,
                    black_box(&refresh_snapshot),
                    black_box(&tcp_target),
                ))
            })
        },
    );
    c.bench_function(
        "authorization/boundary_overhead/8_event_refresh_burst",
        |b| {
            let mut published_generation = 0;
            let mut harness = BoundaryOverheadHarness {
                processed_generation: 0,
            };
            b.iter(|| {
                published_generation += 8;
                black_box(harness.drain_adopt_and_match(
                    published_generation,
                    8,
                    black_box(&refresh_snapshot),
                    black_box(&tcp_target),
                ))
            })
        },
    );

    let mut one_event_generation = 0;
    let mut one_event_harness = BoundaryOverheadHarness {
        processed_generation: 0,
    };
    record_distribution(
        "boundary_overhead/one_generation_refresh",
        || {
            one_event_generation += 1;
            one_event_harness.drain_adopt_and_match(
                one_event_generation,
                1,
                &refresh_snapshot,
                &tcp_target,
            )
        },
        false,
    );
    let mut burst_generation = 0;
    let mut burst_harness = BoundaryOverheadHarness {
        processed_generation: 0,
    };
    record_distribution(
        "boundary_overhead/8_event_refresh_burst",
        || {
            burst_generation += 8;
            burst_harness.drain_adopt_and_match(burst_generation, 8, &refresh_snapshot, &tcp_target)
        },
        false,
    );
}

criterion_group!(benches, bench_authorization);
criterion_main!(benches);

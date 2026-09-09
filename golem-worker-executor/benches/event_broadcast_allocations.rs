// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.

use golem_common::base_model::component::ComponentId;
use golem_common::model::{AgentId, AgentInvocationOutput, AgentInvocationResult, IdempotencyKey};
use golem_common::schema::SchemaValue;
use golem_worker_executor::services::events::{Event, Events};
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct CountingAllocator;

static COUNTING: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn event(payload_size: usize) -> Event {
    Event::InvocationCompleted {
        agent_id: AgentId {
            component_id: ComponentId::new(),
            agent_id: "allocation-benchmark".to_string(),
        },
        idempotency_key: IdempotencyKey::new("target".to_string()),
        result: Ok(AgentInvocationOutput {
            result: AgentInvocationResult::AgentMethod {
                output: SchemaValue::String("x".repeat(payload_size)),
            },
            consumed_fuel: None,
            invocation_status: None,
            component_revision: None,
            agent_id: None,
            idempotency_key: None,
            oplog_index: None,
            agent_fingerprint: None,
        }),
    }
}

fn measure<F>(operation: F) -> usize
where
    F: FnOnce(),
{
    ALLOCATIONS.store(0, Ordering::Relaxed);
    COUNTING.store(true, Ordering::Relaxed);
    operation();
    COUNTING.store(false, Ordering::Relaxed);
    ALLOCATIONS.load(Ordering::Relaxed)
}

fn main() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    for payload_size in [64, 1_048_576] {
        for listener_count in [0, 1, 16] {
            let events = Events::new(32);
            let mut subscriptions = (0..listener_count)
                .map(|_| events.subscribe())
                .collect::<Vec<_>>();
            let event = event(payload_size);
            let allocations = measure(|| {
                events.publish(event);
                runtime.block_on(async {
                    for subscription in &mut subscriptions {
                        subscription
                            .wait_for(|event| {
                                black_box(event);
                                Some(())
                            })
                            .await
                            .unwrap();
                    }
                });
            });
            println!(
                "event_broadcast/unrelated payload_bytes={payload_size} listeners={listener_count} allocations={allocations}"
            );
        }

        let events = Events::new(2);
        let mut subscription = events.subscribe();
        let event = event(payload_size);
        let allocations = measure(|| {
            events.publish(event);
            runtime.block_on(async {
                let output = subscription
                    .wait_for(|event| match event {
                        Event::InvocationCompleted { result, .. } => Some(result.clone()),
                        _ => None,
                    })
                    .await
                    .unwrap()
                    .unwrap();
                black_box(output);
            });
        });
        println!(
            "event_broadcast/matching payload_bytes={payload_size} listeners=1 allocations={allocations}"
        );
    }
}

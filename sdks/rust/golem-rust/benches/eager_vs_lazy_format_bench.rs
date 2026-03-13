// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Microbenchmark: eager format!() vs lazy unwrap_or_else on Result::Ok.
//!
//! This measures the cost difference between:
//!   result.expect(format!("rpc call to {} failed", name).as_str())   // eager: allocates every call
//!   result.unwrap_or_else(|e| panic!("rpc call to {} failed: {:?}", name, e))  // lazy: no alloc on Ok
//!
//! Run with:
//!   cd sdks/rust && cargo test -p golem-rust --test eager_vs_lazy_format_bench -- --nocapture

test_r::enable!();

use std::hint::black_box;
use std::time::Instant;
use test_r::test;

fn bench_loop<F: Fn() -> u64>(label: &str, iterations: usize, f: F) -> std::time::Duration {
    // Warmup
    for _ in 0..1000 {
        black_box(f());
    }

    let start = Instant::now();
    for _ in 0..iterations {
        black_box(f());
    }
    let elapsed = start.elapsed();
    let per_iter = elapsed / iterations as u32;
    println!(
        "  {}: {:?} total, {:?}/iter ({} iterations)",
        label, elapsed, per_iter, iterations
    );
    elapsed
}

/// Simulates what the current generated code does:
///   result.expect(format!("rpc call to {} failed", method_name).as_str())
#[inline(never)]
fn eager_format(result: Result<u64, String>, method_name: &str) -> u64 {
    result.expect(format!("rpc call to {} failed", method_name).as_str())
}

/// Simulates the proposed fix:
///   result.unwrap_or_else(|e| panic!("rpc call to {} failed: {:?}", method_name, e))
#[inline(never)]
fn lazy_format(result: Result<u64, String>, method_name: &str) -> u64 {
    result.unwrap_or_else(|e| panic!("rpc call to {} failed: {:?}", method_name, e))
}

#[test]
fn bench_eager_vs_lazy_format() {
    const ITERATIONS: usize = 10_000_000;

    println!("\n========================================");
    println!("Eager format!() vs Lazy unwrap_or_else");
    println!("========================================\n");

    println!("--- Happy path (Result::Ok) - {} iterations ---", ITERATIONS);
    println!("  This is the hot path: every successful RPC call pays the cost.\n");

    let method_name = "do_something";

    let eager_time = bench_loop("eager  format!().as_str()", ITERATIONS, || {
        let result: Result<u64, String> = Ok(42);
        eager_format(black_box(result), black_box(method_name))
    });

    let lazy_time = bench_loop("lazy   unwrap_or_else    ", ITERATIONS, || {
        let result: Result<u64, String> = Ok(42);
        lazy_format(black_box(result), black_box(method_name))
    });

    let speedup = eager_time.as_nanos() as f64 / lazy_time.as_nanos() as f64;
    println!("\n  Speedup: {:.2}x", speedup);
    println!(
        "  Saved per call: {:?}",
        eager_time
            .checked_sub(lazy_time)
            .map(|d| d / ITERATIONS as u32)
            .unwrap_or_default()
    );

    // Also test with a longer method name (more formatting work)
    let long_method_name = "my_very_long_agent_method_name_with_many_words";
    println!(
        "\n--- Happy path with longer method name ({}) ---\n",
        long_method_name
    );

    let eager_time2 = bench_loop("eager  format!().as_str()", ITERATIONS, || {
        let result: Result<u64, String> = Ok(42);
        eager_format(black_box(result), black_box(long_method_name))
    });

    let lazy_time2 = bench_loop("lazy   unwrap_or_else    ", ITERATIONS, || {
        let result: Result<u64, String> = Ok(42);
        lazy_format(black_box(result), black_box(long_method_name))
    });

    let speedup2 = eager_time2.as_nanos() as f64 / lazy_time2.as_nanos() as f64;
    println!("\n  Speedup: {:.2}x", speedup2);
    println!(
        "  Saved per call: {:?}",
        eager_time2
            .checked_sub(lazy_time2)
            .map(|d| d / ITERATIONS as u32)
            .unwrap_or_default()
    );

    // Simulate the trigger path too (fire-and-forget, unit result)
    println!("\n--- Trigger path (Result<(), String>) ---\n");

    let eager_trigger = bench_loop("eager  format!().as_str()", ITERATIONS, || {
        let result: Result<(), String> = Ok(());
        result.expect(format!("rpc call to trigger {} failed", black_box(method_name)).as_str());
        0u64
    });

    let lazy_trigger = bench_loop("lazy   unwrap_or_else    ", ITERATIONS, || {
        let result: Result<(), String> = Ok(());
        let mn = black_box(method_name);
        result.unwrap_or_else(|e| panic!("rpc call to trigger {} failed: {:?}", mn, e));
        0u64
    });

    let speedup3 = eager_trigger.as_nanos() as f64 / lazy_trigger.as_nanos() as f64;
    println!("\n  Speedup: {:.2}x", speedup3);
    println!(
        "  Saved per call: {:?}",
        eager_trigger
            .checked_sub(lazy_trigger)
            .map(|d| d / ITERATIONS as u32)
            .unwrap_or_default()
    );

    println!();
}

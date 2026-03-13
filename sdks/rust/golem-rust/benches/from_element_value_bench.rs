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

//! Microbenchmark for Schema::from_element_value deserialization path.
//!
//! Measures the cost of from_element_value which is called on every agent RPC
//! response deserialization. The key bottleneck is that T::get_type() rebuilds
//! a full WitType tree that is then immediately discarded.
//!
//! Run with:
//!   cd sdks/rust && cargo test -p golem-rust --features export_golem_agentic --test from_element_value_bench -- --nocapture

test_r::enable!();

#[cfg(feature = "export_golem_agentic")]
mod bench {
    use golem_rust::agentic::Schema;
    use golem_rust::golem_agentic::golem::agent::common::ElementValue;
    use golem_rust::value_and_type::IntoValue;
    use golem_rust_macro::{FromValueAndType, IntoValue};
    use std::hint::black_box;
    use std::time::Instant;
    use test_r::test;

    fn bench_loop<F: Fn()>(label: &str, iterations: usize, f: F) -> std::time::Duration {
        // Warmup
        for _ in 0..100 {
            f();
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

    // Simple type: a single string
    fn make_string_element_value() -> ElementValue {
        ElementValue::ComponentModel("hello world".to_string().into_value())
    }

    // Medium type: a struct with several fields
    #[derive(IntoValue, FromValueAndType, Clone, Debug)]
    struct MediumStruct {
        name: String,
        age: u32,
        active: bool,
        score: f64,
        tags: Vec<String>,
    }

    fn make_medium_struct() -> MediumStruct {
        MediumStruct {
            name: "test user".to_string(),
            age: 42,
            active: true,
            score: 99.5,
            tags: vec!["a".to_string(), "b".to_string(), "c".to_string()],
        }
    }

    fn make_medium_element_value() -> ElementValue {
        ElementValue::ComponentModel(make_medium_struct().into_value())
    }

    // Complex type: nested structs and collections
    #[derive(IntoValue, FromValueAndType, Clone, Debug)]
    struct Address {
        street: String,
        city: String,
        zip: String,
        country: String,
    }

    #[derive(IntoValue, FromValueAndType, Clone, Debug)]
    struct ComplexStruct {
        id: u64,
        name: String,
        description: String,
        addresses: Vec<Address>,
        metadata: Vec<(String, String)>,
        status: Option<String>,
        priority: u32,
        score: f64,
    }

    fn make_complex_struct() -> ComplexStruct {
        ComplexStruct {
            id: 12345,
            name: "Complex Entity".to_string(),
            description: "A complex structure for benchmarking".to_string(),
            addresses: vec![
                Address {
                    street: "123 Main St".to_string(),
                    city: "Springfield".to_string(),
                    zip: "62701".to_string(),
                    country: "US".to_string(),
                },
                Address {
                    street: "456 Oak Ave".to_string(),
                    city: "Shelbyville".to_string(),
                    zip: "62702".to_string(),
                    country: "US".to_string(),
                },
            ],
            metadata: vec![
                ("key1".to_string(), "value1".to_string()),
                ("key2".to_string(), "value2".to_string()),
                ("key3".to_string(), "value3".to_string()),
            ],
            status: Some("active".to_string()),
            priority: 5,
            score: 87.3,
        }
    }

    fn make_complex_element_value() -> ElementValue {
        ElementValue::ComponentModel(make_complex_struct().into_value())
    }

    #[test]
    fn bench_from_element_value() {
        const ITERATIONS: usize = 100_000;

        println!("\n========================================");
        println!("Schema::from_element_value Benchmarks");
        println!("========================================\n");

        // --- Simple type (String) ---
        println!("--- Simple type (String) ---");
        let ev = make_string_element_value();
        bench_loop("from_element_value<String>", ITERATIONS, || {
            let v = ev.clone();
            black_box(String::from_element_value(v).unwrap());
        });

        // --- Medium type (struct with 5 fields) ---
        println!("\n--- Medium type (MediumStruct: 5 fields) ---");
        let ev = make_medium_element_value();
        bench_loop("from_element_value<MediumStruct>", ITERATIONS, || {
            let v = ev.clone();
            black_box(MediumStruct::from_element_value(v).unwrap());
        });

        // --- Complex type (nested struct with collections) ---
        println!("\n--- Complex type (ComplexStruct: nested, collections) ---");
        let ev = make_complex_element_value();
        bench_loop("from_element_value<ComplexStruct>", ITERATIONS, || {
            let v = ev.clone();
            black_box(ComplexStruct::from_element_value(v).unwrap());
        });

        // --- Batch: 10 consecutive deserializations (simulates multi-param) ---
        println!("\n--- Batch: 10x MediumStruct deserializations ---");
        let ev = make_medium_element_value();
        bench_loop("10x from_element_value<MediumStruct>", ITERATIONS, || {
            for _ in 0..10 {
                let v = ev.clone();
                black_box(MediumStruct::from_element_value(v).unwrap());
            }
        });

        println!();
    }
}

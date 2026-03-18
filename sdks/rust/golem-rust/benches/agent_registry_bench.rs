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

//! Microbenchmark for agent registry parameter type lookups.
//!
//! Measures the cost of get_method_parameter_type and get_constructor_parameter_type
//! which are called per-parameter during every agent invoke/initiate call.
//!
//! Run with:
//!   cd sdks/rust && cargo test -p golem-rust --features export_golem_agentic --test agent_registry_bench -- --nocapture

test_r::enable!();

#[cfg(feature = "export_golem_agentic")]
mod bench {
    use golem_rust::agentic::{
        get_constructor_parameter_type, get_enriched_agent_type_by_name, get_method_parameter_type,
        register_agent_type, AgentTypeName, EnrichedAgentMethod, EnrichedElementSchema,
        ExtendedAgentConstructor, ExtendedAgentType, ExtendedDataSchema,
    };
    use golem_rust::golem_agentic::golem::agent::common::{AgentMode, ElementSchema, Snapshotting};
    use golem_wasm::golem_core_1_5_x::types::{NamedWitTypeNode, WitType, WitTypeNode};
    use std::hint::black_box;
    use std::time::Instant;
    use test_r::test;

    fn make_element_schema() -> ElementSchema {
        ElementSchema::ComponentModel(WitType {
            nodes: vec![NamedWitTypeNode {
                name: None,
                owner: None,
                type_: WitTypeNode::PrimStringType,
            }],
        })
    }

    fn make_method(name: &str, param_count: usize) -> EnrichedAgentMethod {
        let params: Vec<(String, EnrichedElementSchema)> = (0..param_count)
            .map(|i| {
                (
                    format!("param_{}", i),
                    EnrichedElementSchema::ElementSchema(make_element_schema()),
                )
            })
            .collect();

        EnrichedAgentMethod {
            name: name.to_string(),
            description: format!("Method {}", name),
            http_endpoint: vec![],
            prompt_hint: Some("hint".to_string()),
            input_schema: ExtendedDataSchema::Tuple(params),
            output_schema: ExtendedDataSchema::Tuple(vec![(
                "result".to_string(),
                EnrichedElementSchema::ElementSchema(make_element_schema()),
            )]),
        }
    }

    fn register_test_agent(name: &str, method_count: usize, params_per_method: usize) {
        let methods: Vec<EnrichedAgentMethod> = (0..method_count)
            .map(|i| make_method(&format!("method_{}", i), params_per_method))
            .collect();

        let constructor_params: Vec<(String, EnrichedElementSchema)> = (0..params_per_method)
            .map(|i| {
                (
                    format!("ctor_param_{}", i),
                    EnrichedElementSchema::ElementSchema(make_element_schema()),
                )
            })
            .collect();

        let agent_type = ExtendedAgentType {
            type_name: name.to_string(),
            description: "Benchmark test agent".to_string(),
            source_language: "rust".to_string(),
            constructor: ExtendedAgentConstructor {
                name: Some("new".to_string()),
                description: "Constructor".to_string(),
                prompt_hint: None,
                input_schema: ExtendedDataSchema::Tuple(constructor_params),
            },
            methods,
            dependencies: vec![],
            mode: AgentMode::Durable,
            http_mount: None,
            snapshotting: Snapshotting::Disabled,
            config: vec![],
        };

        register_agent_type(AgentTypeName(name.to_string()), agent_type);
    }

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

    #[test]
    fn bench_agent_registry_lookups() {
        const METHODS: usize = 10;
        const PARAMS: usize = 5;
        const ITERATIONS: usize = 100_000;

        register_test_agent("BenchAgent", METHODS, PARAMS);

        // Pre-allocate once (simulates the hoisted macro pattern)
        let agent_type_name = AgentTypeName("BenchAgent".to_string());

        println!("\n========================================");
        println!("Agent Registry Lookup Benchmarks");
        println!("Agent: {} methods, {} params each", METHODS, PARAMS);
        println!("========================================\n");

        // --- get_method_parameter_type (hoisted AgentTypeName) ---
        println!("--- get_method_parameter_type ---");

        bench_loop("single param lookup (hoisted name)", ITERATIONS, || {
            get_method_parameter_type(&agent_type_name, "method_5", 2);
        });

        bench_loop(
            &format!("all {} params of one method (hoisted name)", PARAMS),
            ITERATIONS,
            || {
                for i in 0..PARAMS {
                    black_box(get_method_parameter_type(&agent_type_name, "method_5", i));
                }
            },
        );

        // Simulates old pattern: allocate AgentTypeName per parameter
        bench_loop(
            "single param (alloc AgentTypeName each time - old pattern)",
            ITERATIONS,
            || {
                let name = AgentTypeName("BenchAgent".to_string());
                black_box(get_method_parameter_type(&name, "method_5", 2));
            },
        );

        // --- get_constructor_parameter_type ---
        println!("\n--- get_constructor_parameter_type ---");

        bench_loop("single ctor param (hoisted name)", ITERATIONS, || {
            get_constructor_parameter_type(&agent_type_name, 2);
        });

        bench_loop(
            &format!("all {} ctor params (hoisted name)", PARAMS),
            ITERATIONS,
            || {
                for i in 0..PARAMS {
                    black_box(get_constructor_parameter_type(&agent_type_name, i));
                }
            },
        );

        // --- get_enriched_agent_type_by_name (full clone, for comparison) ---
        println!("\n--- get_enriched_agent_type_by_name (full deep clone, for comparison) ---");

        bench_loop("full ExtendedAgentType clone", ITERATIONS, || {
            black_box(get_enriched_agent_type_by_name(&agent_type_name));
        });

        println!();
    }
}

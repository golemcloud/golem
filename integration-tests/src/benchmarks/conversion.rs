use crate::benchmarks::{cleanup_user_state, delete_workers, invoke_and_await_agent};
use async_trait::async_trait;
use futures_concurrency::future::Join;
use golem_common::model::AgentId;
use golem_common::model::agent::ParsedAgentId;
use golem_common::model::component::ComponentDto;
use golem_common::model::environment::EnvironmentId;
use golem_common::schema::{SchemaValue, TypedSchemaValue};
use golem_common::{agent_id, data_value};
use golem_test_framework::benchmark::{
    Benchmark, BenchmarkRecorder, BenchmarkResultValue, RunConfig,
};
use golem_test_framework::config::benchmark::TestMode;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use std::time::Instant;
use tracing::Level;

pub const FIXTURES: [(&str, &str, &str); 5] = [
    (
        "rust",
        "conversion_bench_rust_release",
        "ConversionBenchRust",
    ),
    ("ts", "conversion_bench_ts", "ConversionBenchTs"),
    ("effect", "conversion_bench_effect", "ConversionBenchEffect"),
    ("scala", "conversion_bench_scala", "ConversionBenchScala"),
    (
        "moonbit",
        "conversion_bench_moonbit",
        "ConversionBenchMoonbit",
    ),
];

const STRUCTURAL_FIXTURES: [(&str, &str, &str); 2] = [
    (
        "ts-structural",
        "conversion_bench_ts_structural",
        "ConversionBenchTsStructural",
    ),
    ("effect", "conversion_bench_effect", "ConversionBenchEffect"),
];

pub struct Conversion<const OUTPUT: bool, const STRUCTURAL: bool = false> {
    config: RunConfig,
}

pub struct Iteration {
    user: TestUserContext<BenchmarkTestDependencies>,
    env_id: EnvironmentId,
    components: Vec<(&'static str, ComponentDto, Vec<ParsedAgentId>)>,
}

fn payload<const OUTPUT: bool>(length: usize) -> TypedSchemaValue {
    if OUTPUT {
        data_value!(u32::try_from(length).expect("length exceeds U32"))
    } else {
        let bytes: Vec<u8> = (0..length).map(|i| (i % 251) as u8).collect();
        data_value!(bytes)
    }
}

fn expected<const OUTPUT: bool>(length: usize) -> Vec<SchemaValue> {
    if OUTPUT {
        vec![SchemaValue::List {
            elements: (0..length)
                .map(|i| SchemaValue::U8((i % 251) as u8))
                .collect(),
        }]
    } else {
        // Full periods sum to 0 + ... + 250; derive independently of the guest fold.
        let periods = length / 251;
        let tail = length % 251;
        let sum = periods as u64 * 31_375 + (tail as u64 * tail.saturating_sub(1) as u64) / 2;
        vec![SchemaValue::U32(sum as u32)]
    }
}

#[async_trait]
impl<const OUTPUT: bool, const STRUCTURAL: bool> Benchmark for Conversion<OUTPUT, STRUCTURAL> {
    type BenchmarkContext = BenchmarkTestDependencies;
    type IterationContext = Iteration;

    fn name() -> &'static str {
        match (STRUCTURAL, OUTPUT) {
            (false, false) => "conversion-large-input",
            (false, true) => "conversion-large-output",
            (true, false) => "conversion-structural-large-input",
            (true, true) => "conversion-structural-large-output",
        }
    }

    fn description() -> &'static str {
        if STRUCTURAL {
            "Opt-in TS structural number[] and Effect REST invocation, separate from typed-array marker cases. Independent List<U8> input/checksum and U32 input/List<U8> output; 3 warmups, 9 calls per worker. Client total retains preparation/retry/transport/decoding."
        } else {
            "Opt-in all-five-SDK REST invocation. Independent List<U8> input/checksum and U32 input/List<U8> output; 3 warmups, 9 calls per worker. Total client time includes native preparation and existing invocation retry/JSON work. SDKs run sequentially; size workers run concurrently."
        }
    }

    async fn create_benchmark_context(
        mode: &TestMode,
        verbosity: Level,
        cluster_size: usize,
        disable_compilation_cache: bool,
        otlp: bool,
    ) -> BenchmarkResultValue<Self::BenchmarkContext> {
        Ok(BenchmarkTestDependencies::new(
            mode,
            verbosity,
            cluster_size,
            disable_compilation_cache,
            otlp,
        )
        .await)
    }

    async fn cleanup(context: Self::BenchmarkContext) -> BenchmarkResultValue {
        context.kill_all().await;
        Ok(())
    }

    async fn create(_mode: &TestMode, config: RunConfig) -> BenchmarkResultValue<Self> {
        Ok(Self { config })
    }

    async fn setup_iteration(
        &self,
        deps: &Self::BenchmarkContext,
        _recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue<Iteration> {
        let user = deps.user().await.unwrap();
        let (_, env) = user.app_and_env().await.unwrap();
        let mut components = Vec::new();
        let fixtures: &[_] = if STRUCTURAL {
            &STRUCTURAL_FIXTURES
        } else {
            &FIXTURES
        };
        for &(language, artifact, agent_type) in fixtures {
            let component = user
                .component(&env.id, artifact)
                .name(&format!("conversion-bench:{language}"))
                .store()
                .await
                .unwrap();
            let ids = (0..self.config.size)
                .map(|i| agent_id!(agent_type, format!("conversion-{i}")))
                .collect();
            components.push((language, component, ids));
        }
        Ok(Iteration {
            user,
            env_id: env.id,
            components,
        })
    }

    async fn warmup(
        &self,
        _deps: &Self::BenchmarkContext,
        context: &Iteration,
    ) -> BenchmarkResultValue {
        let method = if OUTPUT { "produce" } else { "checksum" };
        let expected = expected::<OUTPUT>(self.config.length);
        for (_, component, ids) in &context.components {
            for id in ids {
                for _ in 0..3 {
                    let result = invoke_and_await_agent(
                        &context.user,
                        component,
                        id,
                        method,
                        payload::<OUTPUT>(self.config.length),
                    )
                    .await;
                    assert_eq!(result.value, expected);
                }
            }
        }
        Ok(())
    }

    async fn run(
        &self,
        _deps: &Self::BenchmarkContext,
        context: &Iteration,
        recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue {
        let method = if OUTPUT { "produce" } else { "checksum" };
        let expected = expected::<OUTPUT>(self.config.length);
        for (language, component, ids) in &context.components {
            ids.iter()
                .map(|id| {
                    let recorder = &recorder;
                    let expected = &expected;
                    async move {
                        for _ in 0..9 {
                            let start = Instant::now();
                            let params = payload::<OUTPUT>(self.config.length);
                            let preparation = start.elapsed();
                            let result = invoke_and_await_agent(
                                &context.user,
                                component,
                                id,
                                method,
                                params,
                            )
                            .await;
                            let total = start.elapsed();
                            assert_eq!(&result.value, expected);
                            let prefix = format!("{language}-rest-");
                            result.record(recorder, &prefix, &id.to_string());
                            let suffix = if result.failures.is_empty() {
                                "client-total"
                            } else {
                                "client-total-recovered"
                            };
                            recorder.duration(&format!("{prefix}{suffix}").into(), total);
                            recorder.duration(
                                &format!("{prefix}client-native-preparation").into(),
                                preparation,
                            );
                        }
                    }
                })
                .collect::<Vec<_>>()
                .join()
                .await;
        }
        Ok(())
    }

    async fn cleanup_iteration(
        &self,
        _deps: &Self::BenchmarkContext,
        context: Iteration,
        recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue {
        for (_, component, ids) in context.components {
            let ids: Vec<_> = ids
                .iter()
                .map(|id| AgentId::from_agent_id(component.id, id).unwrap())
                .collect();
            delete_workers(&context.user, &ids, &recorder).await;
        }
        cleanup_user_state(&context.user, &context.env_id, &recorder).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn conversion_fixture_expectations_cross_byte_period() {
        for (length, sum) in [
            (0, 0),
            (100, 4_950),
            (250, 31_125),
            (251, 31_375),
            (252, 31_375),
            (10_000, 1_245_780),
        ] {
            assert_eq!(expected::<false>(length), vec![SchemaValue::U32(sum)]);
        }
        let SchemaValue::List { elements } = expected::<true>(252).remove(0) else {
            panic!("output must remain List<U8>");
        };
        assert_eq!(elements.len(), 252);
        assert_eq!(
            &elements[249..],
            &[
                SchemaValue::U8(249),
                SchemaValue::U8(250),
                SchemaValue::U8(0)
            ]
        );
        assert_eq!(
            payload::<true>(10_000).value(),
            &SchemaValue::Record {
                fields: vec![SchemaValue::U32(10_000)]
            }
        );
    }
}

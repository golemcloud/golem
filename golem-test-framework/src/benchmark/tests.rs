use super::*;
use crate::config::BenchmarkCliParameters;
use clap::Parser;
use test_r::test;

struct FailingBenchmark(usize);

#[async_trait]
impl Benchmark for FailingBenchmark {
    type BenchmarkContext = Mutex<Vec<&'static str>>;
    type IterationContext = ();

    fn name() -> &'static str {
        "failing"
    }
    fn description() -> &'static str {
        "Exercises phase failures without services"
    }

    async fn create_benchmark_context(
        _: &TestMode,
        _: Level,
        _: usize,
        _: bool,
        _: bool,
    ) -> BenchmarkResultValue<Self::BenchmarkContext> {
        Err(BenchmarkError::new("setup-context", "context failed"))
    }
    async fn cleanup(_: Self::BenchmarkContext) -> BenchmarkResultValue {
        Ok(())
    }
    async fn create(_: &TestMode, config: RunConfig) -> BenchmarkResultValue<Self> {
        Ok(Self(config.size))
    }
    async fn setup_iteration(
        &self,
        context: &Self::BenchmarkContext,
        recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue<()> {
        context.lock().unwrap().push("setup");
        recorder.count(&ResultKey::primary("seeded-sessions"), 0);
        if self.0 == 0 {
            Err(BenchmarkError::new("setup", "setup failed"))
        } else {
            Ok(())
        }
    }
    async fn warmup(&self, context: &Self::BenchmarkContext, _: &()) -> BenchmarkResultValue {
        context.lock().unwrap().push("warmup");
        assert_ne!(self.0, 1, "warmup panicked");
        Ok(())
    }
    async fn run(
        &self,
        context: &Self::BenchmarkContext,
        _: &(),
        recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue {
        context.lock().unwrap().push("run");
        if self.0 == 4 {
            let index = context
                .lock()
                .unwrap()
                .iter()
                .filter(|event| **event == "run")
                .count()
                - 1;
            if index == 1 {
                return Err(BenchmarkError::new("run", "failed middle iteration"));
            }
            recorder.duration(
                &ResultKey::primary("complete"),
                Duration::from_millis(11 + 13 * index as u64),
            );
            return Ok(());
        }
        if self.0 == 2 {
            return Err(BenchmarkError::new("correctness", "wrong item"));
        }
        recorder.duration(&ResultKey::primary("complete"), Duration::from_millis(7));
        Ok(())
    }
    async fn cleanup_iteration(
        &self,
        context: &Self::BenchmarkContext,
        _: (),
        _: BenchmarkRecorder,
    ) -> BenchmarkResultValue {
        context.lock().unwrap().push("cleanup");
        if self.0 == 4 {
            Ok(())
        } else {
            Err(BenchmarkError::new("cleanup", "cleanup failed"))
        }
    }
}

#[test]
async fn benchmark_phase_failures_skip_measurement_and_always_cleanup_context() {
    let params =
        BenchmarkCliParameters::parse_from(["benchmarks", "benchmark", "failing", "spawned"]);
    for (size, events, failures) in [
        (0, vec!["setup"], 1),
        (1, vec!["setup", "warmup", "cleanup"], 2),
        (2, vec!["setup", "warmup", "run", "cleanup"], 2),
        (3, vec!["setup", "warmup", "run", "cleanup"], 1),
    ] {
        let context = Mutex::new(Vec::new());
        let config = RunConfig {
            cluster_size: 1,
            size,
            length: 4,
            disable_compilation_cache: false,
        };
        let result = run_benchmark::<FailingBenchmark>(
            &context,
            params.benchmark_config.mode(),
            config,
            1,
            1,
            "1/1",
        )
        .await;
        assert_eq!(*context.lock().unwrap(), events);
        assert_eq!(result.failure_count(), failures);
        assert_eq!(result.duration_results.is_empty(), size != 3);
        assert_eq!(
            result.count_results[&ResultKey::primary("seeded-sessions")].all,
            vec![0]
        );
    }
}

#[test]
async fn benchmark_context_failure_is_a_result_and_deadlines_are_failures() {
    let params =
        BenchmarkCliParameters::parse_from(["benchmarks", "benchmark", "failing", "spawned"]);
    let item = BenchmarkSuiteItem {
        name: "failing".into(),
        version: 1,
        iterations: 1,
        partition: None,
        cluster_size: vec![1],
        size: vec![0],
        length: vec![4],
        disable_compilation_cache: None,
    };
    let result = FailingBenchmark::run_benchmark(
        params.benchmark_config.mode(),
        Level::WARN,
        &item,
        true,
        true,
        true,
        false,
    )
    .await;
    assert_eq!(result.failure_count(), 1);
    assert!(result.results[0].duration_results.is_empty());
    let recorder = BenchmarkRecorder::new();
    let value = phase::<()>(
        &recorder,
        "cleanup",
        Duration::from_millis(1),
        std::future::pending(),
    )
    .await;
    assert!(value.is_none());
    assert_eq!(recorder.failures()[&ResultKey::primary("cleanup")].len(), 1);
}

#[test]
async fn three_iterations_preserve_failed_middle_position_and_cleanup() {
    let params =
        BenchmarkCliParameters::parse_from(["benchmarks", "benchmark", "failing", "spawned"]);
    let context = Mutex::new(Vec::new());
    let result = run_benchmark::<FailingBenchmark>(
        &context,
        params.benchmark_config.mode(),
        RunConfig {
            cluster_size: 1,
            size: 4,
            length: 3,
            disable_compilation_cache: false,
        },
        1,
        3,
        "1/1",
    )
    .await;
    assert_eq!(
        *context.lock().unwrap(),
        vec![
            "setup", "warmup", "run", "cleanup", "setup", "warmup", "run", "cleanup", "setup",
            "warmup", "run", "cleanup"
        ]
    );
    let parsed: BenchmarkRunResult =
        serde_json::from_str(&serde_json::to_string(&result).unwrap()).unwrap();
    let key = ResultKey::primary("complete");
    assert_eq!(
        parsed
            .iteration_results
            .iter()
            .map(|iteration| iteration.index)
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert_eq!(
        parsed.iteration_results[0].duration_samples[&key],
        vec![11.0]
    );
    assert!(parsed.iteration_results[1].duration_samples.is_empty());
    assert!(
        parsed.iteration_results[1]
            .failures
            .values()
            .flatten()
            .any(|message| message.contains("failed middle"))
    );
    assert_eq!(
        parsed.iteration_results[2].duration_samples[&key],
        vec![37.0]
    );
    assert_eq!(
        parsed.duration_results[&key].all,
        vec![Duration::from_millis(11), Duration::from_millis(37)]
    );
    assert_eq!(parsed.failure_count(), 1);
}

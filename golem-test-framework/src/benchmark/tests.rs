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
        Err(BenchmarkError::new("cleanup", "cleanup failed"))
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
        iterations: 1,
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

fn suite(file: &str) -> super::config::BenchmarkSuite {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../integration-tests/benchmark_suites")
        .join(file);
    serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

#[test]
fn rust_only_cloud_perf_suite_is_cloud_perf_without_ts_agents() {
    let full = suite("cloud-perf.yaml");
    let rust = suite("cloud-perf-rust.yaml");
    assert!(!full.skip_ts_agents);
    assert!(rust.skip_ts_agents);

    // The same benchmarks and run configs, except the two that use only the TS agent.
    let without_ts_only: Vec<String> = full
        .benchmarks
        .iter()
        .filter(|b| !b.name.ends_with("-medium"))
        .map(|b| serde_yaml::to_string(b).unwrap())
        .collect();
    let rust_items: Vec<String> = rust
        .benchmarks
        .iter()
        .map(|b| serde_yaml::to_string(b).unwrap())
        .collect();
    assert_eq!(rust_items, without_ts_only);
}

#[test]
fn ingress_load_suite_has_its_settings() {
    let ingress = suite("ingress-load.yaml");
    let settings = ingress
        .ingress
        .expect("the suite sets the ingress settings");
    assert_eq!(settings.step_secs, 720);
    assert_eq!(settings.http1_percent, 30);
    assert_eq!(ingress.benchmarks[0].name, "ingress-load");

    // A suite without the field gets no settings, so the benchmark uses the defaults.
    assert!(suite("cloud-perf-rust.yaml").ingress.is_none());
    // Without a mix, every request calls echo.
    assert_eq!(settings.mix, IngressSettings::default().mix);
    assert_eq!(settings.mix[0].method, IngressMethod::Echo);
}

#[test]
fn ingress_load_network_suite_calls_no_executor() {
    let suite = suite("ingress-load-network.yaml");
    let settings = suite.ingress.expect("the suite sets the ingress settings");
    assert_eq!(settings.mix.len(), 1);
    assert_eq!(settings.mix[0].method, IngressMethod::NoInvocation);
    assert_eq!(settings.step_secs, 300);
    assert_eq!(
        suite.benchmarks[0].length,
        vec![2000, 4000, 6000, 8000, 10000]
    );
}

#[test]
fn ingress_load_network_ab_suite_has_the_settings_of_the_network_suite() {
    let network = suite("ingress-load-network.yaml");
    let ab = suite("ingress-load-network-ab.yaml");
    assert_eq!(ab.ingress, network.ingress);
    assert_eq!(ab.benchmarks[0].size, network.benchmarks[0].size);
    assert_eq!(ab.benchmarks[0].length, vec![6000, 8000]);
}

#[test]
fn ingress_load_mix_suite_has_its_mix() {
    let mix = suite("ingress-load-mix.yaml")
        .ingress
        .expect("the suite sets the ingress settings")
        .mix;
    let methods = mix
        .iter()
        .map(|entry| (entry.method, entry.weight, entry.millis, entry.length))
        .collect::<Vec<_>>();
    assert_eq!(
        methods,
        vec![
            (IngressMethod::Echo, 50, None, None),
            (IngressMethod::Sleep, 20, Some(1000), None),
            (IngressMethod::OplogHeavy, 20, None, Some(10)),
            (IngressMethod::CpuIntensive, 10, None, Some(100)),
        ]
    );
}

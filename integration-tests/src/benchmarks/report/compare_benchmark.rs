use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fs::File;
use std::io::BufReader;
use clap::Parser;
use golem_test_framework::dsl::benchmark::{BenchmarkResult, RunConfig};


fn load_json(file_path: &str) -> Result<BenchmarkResult, Box<dyn Error>> {
    let file = File::open(file_path)?;
    let reader = BufReader::new(file);
    let data: BenchmarkResult = serde_json::from_reader(reader)?;
    dbg!(data.clone());
    Ok(data)
}

fn calculate_mean_avg_time(json: &BenchmarkResult) -> HashMap<RunConfig, u64> {
    let mut total_avg_secs = 0;

    let mut run_config_to_avg_time = HashMap::new();


    for (run_config, benchmark_result) in &json.results {

        for duration in benchmark_result.duration_results.values() {
            total_avg_secs += duration.avg.as_secs();
        }

       let avg = total_avg_secs / benchmark_result.duration_results.values().len() as u64;
        run_config_to_avg_time.insert(run_config.clone(), avg);
    }

    run_config_to_avg_time
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct ComparisonResults {
    results: Vec<ComparisonResult>
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct ComparisonResult {
    run_config: RunConfig,
    comparison: Comparison
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct StandaloneResults {
    results: Vec<StandaloneResult>
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct StandaloneResult {
    run_config: RunConfig,
    avg_time: u64
}

impl ComparisonResults {
    fn from_results(previous: &BenchmarkResult, current: &BenchmarkResult) -> Self {
        let previous_avg = calculate_mean_avg_time(previous);
        let current_avg = calculate_mean_avg_time(current);

        let mut comparison_results = Vec::new();

        for (run_config, previous_avg_time) in previous_avg {
            let current_avg_time = current_avg.get(&run_config).unwrap();
            let comparison = Comparison {
                previous_avg: previous_avg_time,
                current_avg: current_avg_time.clone()
            };

            comparison_results.push(ComparisonResult {
                run_config: run_config.clone(),
                comparison
            });
        }

        ComparisonResults {
            results: comparison_results
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Comparison {
    previous_avg: u64,
    current_avg: u64
}

#[derive(Parser, Debug, Clone)]
#[command()]
pub struct CliParams {
    #[arg(long)]
    pub benchmark_previous: Option<String>,
    #[arg(long)]
    pub benchmark_current: String
}

enum BenchmarkReport {
    Single(StandaloneResults),
    ComparisonResult(ComparisonResults),
}

impl BenchmarkReport {
    fn from_benchmark_result(benchmark_result: BenchmarkResult) -> Self {
        let avg_times = calculate_mean_avg_time(&benchmark_result);
        let standalone_results = avg_times.iter().map(|(run_config, avg_time)| {
            StandaloneResult {
                run_config: run_config.clone(),
                avg_time: avg_time.clone()
            }
        }).collect();

        BenchmarkReport::Single(StandaloneResults {
            results: standalone_results
        })
    }

    fn from_results(previous: &BenchmarkResult, current: &BenchmarkResult) -> Self {
        let comparison_result = ComparisonResults::from_results(previous, current);
        BenchmarkReport::ComparisonResult(comparison_result)
    }
}

// The entry point to compare two benchmark results.
// The files have contents expected to be in json format that correspond to `BenchmarkResult`.
fn main() -> Result<(), Box<dyn Error>> {
    let params = CliParams::parse();
    let previous_bench_mark_results = params.benchmark_previous.map(|prev|load_json(prev.as_str()).expect("Failed to read the previous benchmark results possibly due to change in schema"));
    let current_bench_mark_results = load_json(params.benchmark_current.as_str())?;

    let report = if let Some(prev) = previous_bench_mark_results {
        BenchmarkReport::from_results(&prev, &current_bench_mark_results)

    } else {
        BenchmarkReport::from_benchmark_result(current_bench_mark_results)
    };

    match report {
        BenchmarkReport::Single(standalone_results) => {
            println!("{}", serde_json::to_string(&standalone_results)?);
        },
        BenchmarkReport::ComparisonResult(comparison_results) => {
            println!("{}", serde_json::to_string(&comparison_results)?);
        }
    }

    Ok(())
}


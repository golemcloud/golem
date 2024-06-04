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

    let mut hashmap_results = HashMap::new();


    for (run_config, benchmark_result) in &json.results {

        for duration in benchmark_result.duration_results.values() {
            total_avg_secs += duration.avg.as_secs();
        }

       let avg = total_avg_secs / benchmark_result.duration_results.values().len() as u64;
        hashmap_results.insert(run_config.clone(), avg);
    }

    hashmap_results
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct ComparisonResult {
    result: HashMap<RunConfig, Comparison>
}

impl ComparisonResult {
    fn from_results(previous: &BenchmarkResult, current: &BenchmarkResult) -> Self {
        let previous_avg = calculate_mean_avg_time(previous);
        let current_avg = calculate_mean_avg_time(current);

        let mut comparison = HashMap::new();

        for (run_config, avg1) in previous_avg {
            let avg2 = current_avg.get(&run_config).unwrap();
            comparison.insert(run_config, Comparison {
                previous_avg: avg1,
                current_avg: avg2.clone(),
            });
        }

        ComparisonResult {
            result: comparison
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
    pub benchmark_previous: String,
    #[arg(long)]
    pub benchmark_current: String,

}

// The entry point to compare two benchmark results.
// The files have contents expected to be in json format that correspond to `BenchmarkResult`.
fn main() -> Result<(), Box<dyn Error>> {
    let params = CliParams::parse();
    let previous_bench_mark_results = load_json(params.benchmark_previous.as_str())?;
    let current_bench_mark_results = load_json(params.benchmark_current.as_str())?;

    let comparison_result =
        ComparisonResult::from_results(&previous_bench_mark_results, &current_bench_mark_results);

    dbg!(comparison_result.clone());

    println!("{}", serde_json::to_string(&comparison_result)?);

    Ok(())
}


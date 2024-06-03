use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fs::File;
use std::io::BufReader;
use golem_test_framework::dsl::benchmark::{BenchmarkResult, RunConfig};


fn load_json(file_path: &str) -> Result<BenchmarkResult, Box<dyn Error>> {
    let file = File::open(file_path)?;
    let reader = BufReader::new(file);
    let data: BenchmarkResult = serde_json::from_reader(reader)?;
    Ok(data)
}

fn calculate_mean_avg_time(json: &BenchmarkResult) -> HashMap<RunConfig, u64> {
    let mut total_avg_secs = 0;

    let mut hashmap_results = HashMap::new();


    for (run_config, benchmark_result) in &json.results {

        for duration in benchmark_result.duration_results.values() {
            total_avg_secs += duration.avg.as_secs();
            // Add other workers as needed
        }

       let avg = total_avg_secs / benchmark_result.duration_results.values().len() as u64
        hashmap_results.insert(run_config.clone(), avg);
    }

    hashmap_results
}

#[derive(Serialize, Deserialize)]
struct ComparisonResult {
    result: HashMap<RunConfig, Comparison>
}

#[derive(Serialize, Deserialize)]
struct Comparison {
    previous_avg: f64,
    current_avg: f64
}

fn main() -> Result<(), Box<dyn Error>> {
    let file1_path = "file1.json";
    let file2_path = "file2.json";

    let json1 = load_json(file1_path)?;
    let json2 = load_json(file2_path)?;

    let mean_avg_time1 = calculate_mean_avg_time(&json1);
    let mean_avg_time2 = calculate_mean_avg_time(&json2);

    println!("Mean average time of all workers in file1: {}", mean_avg_time1);
    println!("Mean average time of all workers in file2: {}", mean_avg_time2);

    Ok(())
}

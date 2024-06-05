use serde::{Deserialize, Serialize};
use std::error::Error;
use clap::{Args, Parser, Subcommand, ValueEnum};
use golem_test_framework::dsl::benchmark::{BenchmarkResult, RunConfig};
use plotters::prelude::*;
use cli_params::{CliReportParams};
use internal::*;

// The entry point to create reports from benchmarks
// In future, this can be extended to support more complex reports including plotting graphs
fn main() -> Result<(), Box<dyn Error>> {
    let params = CliReportParams::parse();
    match params {
        CliReportParams::CompareBenchmarks(args) => {
            let final_report = BenchmarkComparisonReport::from(args.files)?;
            println!("{}", serde_json::to_string(&final_report)?);
        }
        CliReportParams::GetReport(args) => {
            let final_report = BenchmarkReport::from(args.files)?;

            println!("{}", serde_json::to_string(&final_report)?);

        }
    }

    Ok(())
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct ComparisonForAllRunConfigs {
    results: Vec<ComparisonPerRunConfig>
}
#[derive(Serialize, Deserialize, Clone, Debug)]
struct ComparisonPerRunConfig {
    run_config: RunConfig,
    comparison: Comparison
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct BenchmarkComparisonReport {
    results: Vec<ComparisonReportPerBenchmarkType>
}

impl BenchmarkComparisonReport {
    pub fn from(input: Vec<(BenchmarkType, BenchmarkResultFiles)>) -> Result<BenchmarkComparisonReport, String> {
        let mut comparison_results: Vec<ComparisonReportPerBenchmarkType> = vec![];
        for (benchmark_type, files) in input {
            let previous_bench_mark_results = load_json(files.previous_file.0.as_str())?;
            let current_bench_mark_results = load_json(files.current_file.0.as_str())?;

            let report =
                ComparisonReportPerBenchmarkType {
                    benchmark_type: benchmark_type.clone(),
                    comparison_results: ComparisonForAllRunConfigs::from_results(&previous_bench_mark_results, &current_bench_mark_results)
                };

            comparison_results.push(report);
        }

        Ok(BenchmarkComparisonReport {
            results: comparison_results
        })
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct ComparisonReportPerBenchmarkType {
    benchmark_type: BenchmarkType,
    comparison_results: ComparisonForAllRunConfigs
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct BenchmarkReport {
    results: Vec<BenchmarkReportPerBenchmarkType>
}

impl BenchmarkReport {
    pub fn from(input: Vec<(BenchmarkType, BenchmarkFile)>) -> Result<BenchmarkReport, String> {
        let mut benchmark_results: Vec<BenchmarkReportPerBenchmarkType> = vec![];
        for (benchmark_type, file) in input {
            let current_bench_mark_results = load_json(file.0.as_str())?;

            let run_config_to_avg_time = calculate_mean_avg_time(&current_bench_mark_results);

            let mut report_results = Vec::new();
            for (run_config, avg_time) in run_config_to_avg_time {
                report_results.push(BenchmarkReportPerRunConfig {
                    run_config,
                    avg_time
                });
            }

            let report = BenchmarkReportPerBenchmarkType {
                benchmark_type,
                report: BenchmarkReportForAllRunConfigs {
                    results: report_results
                }
            };

            benchmark_results.push(report);
        }

        Ok(BenchmarkReport {
            results: benchmark_results
        })
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct BenchmarkReportPerBenchmarkType {
    benchmark_type: BenchmarkType,
    report: BenchmarkReportForAllRunConfigs

}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct BenchmarkReportForAllRunConfigs {
    results: Vec<BenchmarkReportPerRunConfig>
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct BenchmarkReportPerRunConfig {
    run_config: RunConfig,
    avg_time: u64
}

impl ComparisonForAllRunConfigs {
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

            comparison_results.push(ComparisonPerRunConfig {
                run_config: run_config.clone(),
                comparison
            });
        }

        ComparisonForAllRunConfigs {
            results: comparison_results
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Comparison {
    previous_avg: u64,
    current_avg: u64
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct BenchmarkType(String);

#[derive(Debug, Clone)]
struct BenchmarkResultFiles {
    previous_file: BenchmarkFile,
    current_file: BenchmarkFile
}

#[derive(Debug, Clone)]
struct BenchmarkFile(String);


mod internal {
    use std::collections::HashMap;
    use std::fs::File;
    use std::io::BufReader;
    use golem_test_framework::dsl::benchmark::{BenchmarkResult, RunConfig};

    pub fn load_json(file_path: &str) -> Result<BenchmarkResult, String> {
        let file = File::open(file_path).map_err(|err| format!("Failed to open file {}. {}", file_path, err))?;
        let reader = BufReader::new(file);
        let data: BenchmarkResult = serde_json::from_reader(reader).map_err(|err| format!("Failed to read JSON from {}. {}", file_path, err))?;
        Ok(data)
    }

    pub fn calculate_mean_avg_time(json: &BenchmarkResult) -> HashMap<RunConfig, u64> {
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

}

mod cli_params {
    use clap::Parser;
    use crate::{BenchmarkType, BenchmarkResultFiles, BenchmarkFile};


    #[derive(Parser)]
    #[command()]
    pub enum CliReportParams {
        CompareBenchmarks(BenchmarkComparisonReportArgs),
        GetReport(BenchmarkReportArgs)
    }


    #[derive(Parser)]
    pub struct BenchmarkComparisonReportArgs {
        #[clap(long, value_parser = parse_comparison_details, value_name="BENCHMARK_TYPE=PREVIOUS_FILE,CURRENT_FILE")]
        pub files: Vec<(BenchmarkType, BenchmarkResultFiles)>,
    }

    #[derive(Parser)]
    pub struct BenchmarkReportArgs {
        #[clap(long, value_parser = parse_benchmark_file, value_name="BENCHMARK_TYPE=FILE_NAME")]
        pub files: Vec<(BenchmarkType, BenchmarkFile)>,
    }


    pub fn parse_comparison_details(
        s: &str,
    ) -> Result<(BenchmarkType, BenchmarkResultFiles), Box<dyn std::error::Error + Send + Sync + 'static>> {
        let pos = s
            .find('=')
            .ok_or_else(|| format!("invalid KEY=value: no `=` found in `{}`", s))?;
        let (label, files_str) = s.split_at(pos);
        let files_str = &files_str[1..]; // skip the '='
        let files: Vec<&str> = files_str.split(",").collect();

        if files.len() != 2 {
            return Err(format!("Expected two files, found {}", files.len()).into());
        }

        match (files.first(), files.get(1)) {
            (Some(&prev), Some(&curr)) => {
                if prev.is_empty() || curr.is_empty() {
                    Err("Empty file names".into())
                } else {
                    Ok((BenchmarkType(label.to_string()), BenchmarkResultFiles {
                        previous_file: BenchmarkFile(prev.trim().to_string()),
                        current_file: BenchmarkFile(curr.trim().to_string())
                    }))
                }
            }
            _ => return Err("Expected two files comma separated. Example: large_storage=file1,file2".into()),
        }
    }

    pub fn parse_benchmark_file(
        s: &str,
    ) -> Result<(BenchmarkType, BenchmarkFile), Box<dyn std::error::Error + Send + Sync + 'static>> {
        let pos = s
            .find('=')
            .ok_or_else(|| format!("invalid KEY=value: no `=` found in `{}`", s))?;
        let (label, files_str) = s.split_at(pos);
        let files_str = &files_str[1..]; // skip the '='

        if files_str.is_empty() {
            return Err("Empty file name".into());
        } else {
            Ok((BenchmarkType(label.to_string()), BenchmarkFile(files_str.trim().to_string())))
        }
    }

}
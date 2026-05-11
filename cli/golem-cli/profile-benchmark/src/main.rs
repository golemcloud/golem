use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Output};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_RUNS: usize = 1;
const DEFAULT_TESTS: &[&str] = &[
    "app::app::app_build_with_rust_component",
    "app::app::basic_ifs_deploy",
    "app::agents::test_rust_counter",
];
const DEFAULT_PROFILES: &[&str] = &[
    "debug",
    "dev-ci",
    "dev-release",
    "dev-release-ci",
    "release",
];
const ALLOWED_PROFILES: &[&str] = &[
    "debug",
    "dev-ci",
    "dev-release",
    "dev-release-ci",
    "release",
];

fn main() {
    if let Err(err) = dispatch() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn dispatch() -> Result<(), String> {
    match env::args().nth(1).as_deref() {
        None | Some("test") => run_test_benchmark(),
        Some("build") => run_build_benchmark(),
        Some(command) => Err(format!(
            "Unsupported benchmark command '{command}'. Expected 'test' or 'build'."
        )),
    }
}

fn run_test_benchmark() -> Result<(), String> {
    if env::var("GOLEM_CLI_TEST_DIR").is_ok_and(|value| !value.trim().is_empty()) {
        return Err(
            "GOLEM_CLI_TEST_DIR is set. Unset it before benchmarking to avoid reusing test state."
                .to_string(),
        );
    }

    let runs = parse_runs("CLI_PROFILE_BENCH_RUNS")?;
    let profiles = parse_profiles()?;
    let tests = env::var("CLI_PROFILE_BENCH_TESTS")
        .ok()
        .map(|value| parse_tests(&value))
        .transpose()?
        .unwrap_or_else(|| DEFAULT_TESTS.iter().map(|test| test.to_string()).collect());

    let output_dir = PathBuf::from("target")
        .join("cli-profile-benchmark")
        .join(timestamp());
    fs::create_dir_all(&output_dir).map_err(|err| {
        format!(
            "Failed to create output directory {}: {err}",
            output_dir.display()
        )
    })?;

    println!("CLI profile benchmark");
    println!();
    println!("runs: {runs}");
    println!("compiled before timing: yes");
    println!("profiles: {}", profiles.join(", "));
    println!("logs: {}", output_dir.display());
    println!();

    compile_before_timing(&profiles, &output_dir)?;

    let mut results = Vec::new();

    for run_idx in 0..runs {
        for profile in profile_order(&profiles, run_idx) {
            for test in &tests {
                let result = run_test(profile, test, run_idx + 1, &output_dir)?;
                print_result_line(&result);
                if !result.success {
                    return Err(format!(
                        "Benchmark test failed: profile={}, test={}, log={}",
                        result.profile,
                        result.test,
                        result.log_path.display()
                    ));
                }
                results.push(result);
            }
        }
    }

    write_csv(&output_dir.join("results.csv"), &results)?;
    let summary = summary_table(runs, &tests, &profiles, &results);
    write_markdown(
        &output_dir.join("results.md"),
        "CLI profile benchmark",
        &[
            ("runs", runs.to_string()),
            ("compiled before timing", "yes".to_string()),
            ("profiles", profiles.join(", ")),
        ],
        &summary,
    )?;
    print_summary(&summary);
    println!();
    println!("Logs: {}", output_dir.display());
    println!("CSV:  {}", output_dir.join("results.csv").display());
    println!("Markdown:  {}", output_dir.join("results.md").display());

    Ok(())
}

fn run_build_benchmark() -> Result<(), String> {
    let runs = parse_runs("CLI_PROFILE_BUILD_BENCH_RUNS")?;
    let profiles = parse_profiles()?;
    let output_dir = PathBuf::from("target")
        .join("cli-profile-build-benchmark")
        .join(timestamp());
    fs::create_dir_all(&output_dir).map_err(|err| {
        format!(
            "Failed to create output directory {}: {err}",
            output_dir.display()
        )
    })?;

    println!("CLI profile build benchmark");
    println!();
    println!("runs: {runs}");
    println!("clean state: isolated CARGO_TARGET_DIR per profile/run");
    println!("profiles: {}", profiles.join(", "));
    println!("logs: {}", output_dir.display());
    println!();

    println!("Preparing WIT dependencies...");
    run_logged_command(
        command("cargo", ["make", "--no-workspace", "wit"]),
        &output_dir.join("prepare-wit.log"),
    )?
    .ensure_success("prepare WIT dependencies")?;
    println!();

    let mut results = Vec::new();
    let scenario = "cli-binaries".to_string();

    for run_idx in 0..runs {
        for profile in profile_order(&profiles, run_idx) {
            let result = run_clean_build(profile, run_idx + 1, &output_dir, &scenario)?;
            print_result_line(&result);
            if !result.success {
                return Err(format!(
                    "Build benchmark failed: profile={}, log={}",
                    result.profile,
                    result.log_path.display()
                ));
            }
            results.push(result);
        }
    }

    write_csv(&output_dir.join("results.csv"), &results)?;
    let summary = summary_table(runs, &[scenario], &profiles, &results);
    write_markdown(
        &output_dir.join("results.md"),
        "CLI profile build benchmark",
        &[
            ("runs", runs.to_string()),
            (
                "clean state",
                "isolated CARGO_TARGET_DIR per profile/run".to_string(),
            ),
            ("profiles", profiles.join(", ")),
        ],
        &summary,
    )?;
    print_summary(&summary);
    println!();
    println!("Logs: {}", output_dir.display());
    println!("CSV:  {}", output_dir.join("results.csv").display());
    println!("Markdown:  {}", output_dir.join("results.md").display());

    Ok(())
}

fn parse_runs(env_var: &str) -> Result<usize, String> {
    let runs = env::var(env_var)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|err| format!("Invalid {env_var} value '{value}': {err}"))
        })
        .transpose()?
        .unwrap_or(DEFAULT_RUNS);

    if runs == 0 {
        Err(format!("{env_var} must be at least 1"))
    } else {
        Ok(runs)
    }
}

fn parse_profiles() -> Result<Vec<String>, String> {
    let profiles = env::var("CLI_PROFILE_BENCH_PROFILES")
        .ok()
        .map(|value| parse_profile_list(&value))
        .transpose()?
        .unwrap_or_else(|| {
            DEFAULT_PROFILES
                .iter()
                .map(|profile| profile.to_string())
                .collect()
        });

    if profiles.is_empty() {
        return Err("CLI_PROFILE_BENCH_PROFILES did not contain any profiles".to_string());
    }

    Ok(profiles)
}

fn parse_profile_list(value: &str) -> Result<Vec<String>, String> {
    let mut profiles = Vec::new();
    for profile in value
        .split(',')
        .map(str::trim)
        .filter(|profile| !profile.is_empty())
    {
        if !ALLOWED_PROFILES.contains(&profile) {
            return Err(format!(
                "Unsupported CLI_PROFILE_BENCH_PROFILES profile '{profile}'. Expected one of: {}",
                ALLOWED_PROFILES.join(", ")
            ));
        }

        if profiles.iter().any(|existing| existing == profile) {
            return Err(format!(
                "Duplicate CLI_PROFILE_BENCH_PROFILES profile '{profile}'"
            ));
        }

        profiles.push(profile.to_string());
    }

    Ok(profiles)
}

fn parse_tests(value: &str) -> Result<Vec<String>, String> {
    let tests = value
        .split(',')
        .map(str::trim)
        .filter(|test| !test.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    if tests.is_empty() {
        Err("CLI_PROFILE_BENCH_TESTS did not contain any test filters".to_string())
    } else {
        Ok(tests)
    }
}

fn compile_before_timing(profiles: &[String], output_dir: &Path) -> Result<(), String> {
    for profile in profiles {
        println!("Compiling {profile} CLI test binaries...");
        run_logged_command(
            command_with_env(
                "cargo",
                ["make", "--no-workspace", "build-cli-test-bins"],
                [("GOLEM_CLI_TEST_BIN_PROFILE", profile.as_str())],
            ),
            &output_dir.join(format!("compile-{profile}-bins.log")),
        )?
        .ensure_success(&format!("compile {profile} CLI test binaries"))?;
    }

    println!("Compiling CLI integration test harness...");
    run_logged_command(
        command(
            "cargo",
            [
                "test",
                "--package",
                "golem-cli",
                "--test",
                "integration",
                "--no-run",
            ],
        ),
        &output_dir.join("compile-integration-test.log"),
    )?
    .ensure_success("compile CLI integration test harness")?;

    println!();
    Ok(())
}

fn run_clean_build(
    profile: &str,
    run_number: usize,
    output_dir: &Path,
    scenario: &str,
) -> Result<BenchResult, String> {
    let log_dir = output_dir.join(profile);
    fs::create_dir_all(&log_dir).map_err(|err| {
        format!(
            "Failed to create profile log directory {}: {err}",
            log_dir.display()
        )
    })?;

    let log_path = log_dir.join(format!("build-run-{run_number}.log"));
    let target_dir = output_dir
        .join("targets")
        .join(format!("{}-run-{run_number}", slug(profile)));
    fs::create_dir_all(&target_dir).map_err(|err| {
        format!(
            "Failed to create target directory {}: {err}",
            target_dir.display()
        )
    })?;

    let timed_output = run_logged_commands(build_commands(profile, &target_dir)?, &log_path)?;

    Ok(BenchResult {
        test: scenario.to_string(),
        profile: profile.to_string(),
        run_number,
        duration: timed_output.duration,
        success: timed_output.success,
        log_path,
    })
}

fn build_commands(profile: &str, target_dir: &Path) -> Result<Vec<Command>, String> {
    Ok(vec![
        build_command(profile, "golem-cli", target_dir)?,
        build_command(profile, "golem", target_dir)?,
    ])
}

fn build_command(profile: &str, binary_name: &str, target_dir: &Path) -> Result<Command, String> {
    let mut command = Command::new("cargo");
    command.env("CARGO_TARGET_DIR", target_dir);

    match profile {
        "debug" => {
            command.args(["build", "-p", binary_name, "--bin", binary_name]);
        }
        "dev-ci" => {
            command.args([
                "build",
                "--profile",
                "dev-ci",
                "-p",
                binary_name,
                "--bin",
                binary_name,
            ]);
        }
        "dev-release" => {
            command.env("GOLEM_BUILD_SKIP_SHADOW", "1");
            command.args([
                "build",
                "--profile",
                "dev-release",
                "-p",
                binary_name,
                "--bin",
                binary_name,
            ]);
        }
        "dev-release-ci" => {
            command.env("GOLEM_BUILD_SKIP_SHADOW", "1");
            command.args([
                "build",
                "--profile",
                "dev-release-ci",
                "-p",
                binary_name,
                "--bin",
                binary_name,
            ]);
        }
        "release" => {
            command.env("GOLEM_BUILD_SKIP_SHADOW", "1");
            command.args([
                "build",
                "--release",
                "-p",
                binary_name,
                "--bin",
                binary_name,
            ]);
        }
        _ => return Err(format!("Unsupported build profile '{profile}'")),
    }

    Ok(command)
}

fn run_test(
    profile: &str,
    test: &str,
    run_number: usize,
    output_dir: &Path,
) -> Result<BenchResult, String> {
    let log_dir = output_dir.join(profile);
    fs::create_dir_all(&log_dir).map_err(|err| {
        format!(
            "Failed to create profile log directory {}: {err}",
            log_dir.display()
        )
    })?;

    let log_path = log_dir.join(format!("{}-run-{run_number}.log", slug(test)));

    let command = command_with_env(
        "cargo-test-r",
        [
            "run",
            "--package",
            "golem-cli",
            "--test",
            "integration",
            test,
            "--",
            "--nocapture",
            "--report-time",
            "--test-threads=1",
        ],
        [
            ("GOLEM_CLI_TEST_BIN_PROFILE", profile),
            ("QUIET", "true"),
            ("RUST_LOG", "info"),
            ("RUST_BACKTRACE", "1"),
        ],
    );

    let timed_output = run_logged_command(command, &log_path)?;

    Ok(BenchResult {
        test: test.to_string(),
        profile: profile.to_string(),
        run_number,
        duration: timed_output.duration,
        success: timed_output.status.success(),
        log_path,
    })
}

fn command<I, S>(program: &str, args: I) -> Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new(program);
    command.args(args);
    command
}

fn command_with_env<I, S, E, K, V>(program: &str, args: I, envs: E) -> Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
    E: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let mut command = command(program, args);
    command.envs(envs);
    command
}

fn run_logged_command(mut command: Command, log_path: &Path) -> Result<TimedOutput, String> {
    let start = Instant::now();
    let output = command.output().map_err(|err| {
        format!(
            "Failed to run command '{}' for log {}: {err}",
            command_display(&command),
            log_path.display()
        )
    })?;
    let duration = start.elapsed();

    write_log(log_path, &command, duration, &output)
        .map_err(|err| format!("Failed to write command log {}: {err}", log_path.display()))?;

    Ok(TimedOutput {
        duration,
        status: output.status,
    })
}

fn run_logged_commands(
    mut commands: Vec<Command>,
    log_path: &Path,
) -> Result<TimedCommandsOutput, String> {
    let start = Instant::now();
    let mut command_outputs = Vec::new();
    let mut success = true;

    for command in &mut commands {
        let output = command.output().map_err(|err| {
            format!(
                "Failed to run command '{}' for log {}: {err}",
                command_display(command),
                log_path.display()
            )
        })?;

        if !output.status.success() {
            success = false;
        }

        command_outputs.push((command_display(command), output));

        if !success {
            break;
        }
    }

    let duration = start.elapsed();
    write_commands_log(log_path, duration, &command_outputs)
        .map_err(|err| format!("Failed to write command log {}: {err}", log_path.display()))?;

    Ok(TimedCommandsOutput { duration, success })
}

fn write_log(
    log_path: &Path,
    command: &Command,
    duration: Duration,
    output: &Output,
) -> io::Result<()> {
    let mut content = String::new();
    content.push_str(&format!("command: {}\n", command_display(command)));
    content.push_str(&format!("status: {}\n", output.status));
    content.push_str(&format!("duration_seconds: {:.3}\n", seconds(duration)));
    content.push_str("\n--- stdout ---\n");
    content.push_str(&String::from_utf8_lossy(&output.stdout));
    content.push_str("\n--- stderr ---\n");
    content.push_str(&String::from_utf8_lossy(&output.stderr));
    fs::write(log_path, content)
}

fn write_commands_log(
    log_path: &Path,
    duration: Duration,
    command_outputs: &[(String, Output)],
) -> io::Result<()> {
    let mut content = String::new();
    content.push_str(&format!("duration_seconds: {:.3}\n", seconds(duration)));

    for (idx, (command, output)) in command_outputs.iter().enumerate() {
        content.push_str(&format!("\n=== command {} ===\n", idx + 1));
        content.push_str(&format!("command: {command}\n"));
        content.push_str(&format!("status: {}\n", output.status));
        content.push_str("\n--- stdout ---\n");
        content.push_str(&String::from_utf8_lossy(&output.stdout));
        content.push_str("\n--- stderr ---\n");
        content.push_str(&String::from_utf8_lossy(&output.stderr));
    }

    fs::write(log_path, content)
}

fn write_csv(path: &Path, results: &[BenchResult]) -> Result<(), String> {
    let mut csv = String::from("test,profile,run,duration_seconds,success,log_path\n");
    for result in results {
        csv.push_str(&format!(
            "{},{},{},{:.3},{},{}\n",
            csv_escape(&result.test),
            csv_escape(&result.profile),
            result.run_number,
            seconds(result.duration),
            result.success,
            csv_escape(&result.log_path.display().to_string())
        ));
    }

    fs::write(path, csv)
        .map_err(|err| format!("Failed to write results CSV {}: {err}", path.display()))
}

fn write_markdown(
    path: &Path,
    title: &str,
    details: &[(&str, String)],
    summary: &str,
) -> Result<(), String> {
    let mut content = String::new();
    content.push_str(&format!("## {title}\n\n"));
    for (key, value) in details {
        content.push_str(&format!("- {key}: {value}\n"));
    }
    content.push('\n');
    content.push_str(summary);

    fs::write(path, content)
        .map_err(|err| format!("Failed to write results Markdown {}: {err}", path.display()))
}

fn print_summary(summary: &str) {
    println!();
    println!("Summary");
    println!();
    print!("{summary}");
}

fn summary_table(
    runs: usize,
    tests: &[String],
    profiles: &[String],
    results: &[BenchResult],
) -> String {
    let mut table = String::new();
    let baseline = baseline_profile(profiles);

    if runs == 1 {
        let mut headers = profiles.to_vec();
        headers.extend(speedup_headers(profiles, baseline));
        table.push_str(&format!("| test | {} |\n", headers.join(" | ")));
        push_separator(&mut table, headers.len());
    } else {
        let mut headers = profile_headers(profiles, "mean");
        headers.extend(speedup_headers(profiles, baseline));
        headers.extend(profile_headers(profiles, "min/max"));
        table.push_str(&format!("| test | {} |\n", headers.join(" | ")));
        push_separator(&mut table, headers.len());
    }

    for test in tests {
        let stats_by_profile = profiles
            .iter()
            .map(|profile| (profile.as_str(), stats(results, test, profile)))
            .collect::<Vec<_>>();
        let baseline_stats = stats_by_profile
            .iter()
            .find(|(profile, _)| *profile == baseline)
            .map(|(_, stats)| stats)
            .expect("baseline profile should be present");

        if runs == 1 {
            let mut columns = stats_by_profile
                .iter()
                .map(|(_, stats)| format_duration(stats.mean))
                .collect::<Vec<_>>();
            columns.extend(
                stats_by_profile
                    .iter()
                    .filter(|(profile, _)| *profile != baseline)
                    .map(|(_, stats)| format!("{:.2}x", baseline_stats.mean / stats.mean)),
            );

            table.push_str(&format!("| {} | {} |\n", test, columns.join(" | ")));
        } else {
            let mut columns = stats_by_profile
                .iter()
                .map(|(_, stats)| format_duration(stats.mean))
                .collect::<Vec<_>>();
            columns.extend(
                stats_by_profile
                    .iter()
                    .filter(|(profile, _)| *profile != baseline)
                    .map(|(_, stats)| format!("{:.2}x", baseline_stats.mean / stats.mean)),
            );
            columns.extend(stats_by_profile.iter().map(|(_, stats)| {
                format!(
                    "{} / {}",
                    format_duration(stats.min),
                    format_duration(stats.max)
                )
            }));

            table.push_str(&format!("| {} | {} |\n", test, columns.join(" | ")));
        }
    }

    table
}

fn profile_order(profiles: &[String], run_idx: usize) -> Vec<&str> {
    (0..profiles.len())
        .map(|offset| profiles[(run_idx + offset) % profiles.len()].as_str())
        .collect()
}

fn profile_headers(profiles: &[String], suffix: &str) -> Vec<String> {
    profiles
        .iter()
        .map(|profile| format!("{profile} {suffix}"))
        .collect::<Vec<_>>()
}

fn speedup_headers(profiles: &[String], baseline: &str) -> Vec<String> {
    profiles
        .iter()
        .filter(|profile| profile.as_str() != baseline)
        .map(|profile| format!("{profile} speedup vs {baseline}"))
        .collect::<Vec<_>>()
}

fn baseline_profile(profiles: &[String]) -> &str {
    profiles
        .iter()
        .find(|profile| profile.as_str() == "debug")
        .or_else(|| profiles.first())
        .expect("at least one profile should be configured")
}

fn push_separator(table: &mut String, numeric_columns: usize) {
    table.push_str(&format!(
        "| --- | {} |\n",
        vec!["---:"; numeric_columns].join(" | ")
    ));
}

fn stats(results: &[BenchResult], test: &str, profile: &str) -> Stats {
    let values = results
        .iter()
        .filter(|result| result.test == test && result.profile == profile)
        .map(|result| seconds(result.duration))
        .collect::<Vec<_>>();

    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let mean = values.iter().sum::<f64>() / values.len() as f64;

    Stats { mean, min, max }
}

fn print_result_line(result: &BenchResult) {
    println!(
        "{} run {} {}: {} ({})",
        result.profile,
        result.run_number,
        result.test,
        format_duration(seconds(result.duration)),
        if result.success { "passed" } else { "failed" }
    );
}

fn command_display(command: &Command) -> String {
    let program = command.get_program().to_string_lossy();
    let args = command
        .get_args()
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
    if args.is_empty() {
        program.to_string()
    } else {
        format!("{program} {args}")
    }
}

fn slug(test: &str) -> String {
    test.chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect()
}

fn timestamp() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    seconds.to_string()
}

fn seconds(duration: Duration) -> f64 {
    duration.as_secs_f64()
}

fn format_duration(seconds: f64) -> String {
    format!("{seconds:.3}s")
}

fn csv_escape(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

struct TimedOutput {
    duration: Duration,
    status: ExitStatus,
}

struct TimedCommandsOutput {
    duration: Duration,
    success: bool,
}

impl TimedOutput {
    fn ensure_success(self, action: &str) -> Result<Self, String> {
        if self.status.success() {
            Ok(self)
        } else {
            Err(format!("Failed to {action}: {}", self.status))
        }
    }
}

struct BenchResult {
    test: String,
    profile: String,
    run_number: usize,
    duration: Duration,
    success: bool,
    log_path: PathBuf,
}

struct Stats {
    mean: f64,
    min: f64,
    max: f64,
}

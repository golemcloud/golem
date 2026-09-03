use base64::prelude::*;
use serde::Deserialize;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read};

pub const ISSUE_TITLE: &str = "Known flaky tests";
pub const ISSUE_MARKER: &str = "<!-- known-flaky-tests:v1 -->";
pub const CLEAN_RUNS_TO_RESOLVE: usize = 10;

#[derive(Clone, Debug)]
pub struct Observation {
    pub name: String,
    pub status: String,
    pub duration: f64,
    pub retries: u64,
    pub run_id: u64,
    pub attempt: u32,
    pub branch: String,
    pub seen_at: String,
    pub artifact_name: String,
    pub run_url: String,
    pub job_url: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ArtifactContext {
    pub run_id: u64,
    pub attempt: u32,
    pub branch: String,
    pub seen_at: String,
    pub artifact_name: String,
    pub run_url: String,
}

#[derive(Debug)]
pub struct TestStats {
    pub name: String,
    pub observations: u64,
    pub runs: HashSet<u64>,
    pub failure_runs: HashSet<u64>,
    pub failures: u64,
    pub retries: u64,
    pub retried_runs: HashSet<u64>,
    pub main_failures: u64,
    pub main_failure_runs: HashSet<u64>,
    pub durations: Vec<f64>,
    pub last_flaky: String,
    pub clean_runs_since_flaky: usize,
    pub links: Vec<String>,
    pub flips: u64,
}

impl TestStats {
    pub fn fail_rate(&self) -> f64 {
        if self.runs.is_empty() {
            0.0
        } else {
            self.failure_runs.len() as f64 / self.runs.len() as f64
        }
    }

    pub fn score(&self) -> u64 {
        self.flips * 5
            + self.retried_runs.len() as u64 * 3
            + self.main_failure_runs.len() as u64 * 2
    }

    pub fn is_active(&self) -> bool {
        self.clean_runs_since_flaky < CLEAN_RUNS_TO_RESOLVE
    }
}

#[derive(Default)]
struct RunEvidence {
    seen_at: String,
    statuses: HashMap<u32, HashSet<String>>,
    retries: u64,
    main_failure: bool,
}

impl RunEvidence {
    fn is_flip(&self) -> bool {
        let failed = self
            .statuses
            .iter()
            .filter(|(_, statuses)| statuses.contains("failed"))
            .map(|(attempt, _)| *attempt);
        let passed = self
            .statuses
            .iter()
            .filter(|(_, statuses)| statuses.contains("passed"))
            .map(|(attempt, _)| *attempt)
            .collect::<Vec<_>>();
        failed.into_iter().any(|failed_attempt| {
            passed
                .iter()
                .any(|passed_attempt| failed_attempt < *passed_attempt)
        })
    }

    fn is_flaky_signal(&self) -> bool {
        self.retries > 0 || self.main_failure || self.is_flip()
    }

    fn is_clean(&self) -> bool {
        self.retries == 0
            && self
                .statuses
                .values()
                .any(|statuses| statuses.contains("passed"))
            && !self
                .statuses
                .values()
                .any(|statuses| statuses.contains("failed"))
    }

    fn was_executed(&self) -> bool {
        self.statuses
            .values()
            .any(|statuses| statuses.contains("passed") || statuses.contains("failed"))
    }
}

#[derive(Deserialize)]
struct CtrfReport {
    results: CtrfResults,
}

#[derive(Deserialize)]
struct CtrfResults {
    tests: Vec<CtrfTest>,
}

#[derive(Deserialize)]
struct CtrfTest {
    name: Option<String>,
    status: Option<String>,
    duration: Option<f64>,
    retries: Option<u64>,
    flaky: Option<bool>,
}

pub fn observations_from_archive(
    bytes: &[u8],
    context: &ArtifactContext,
) -> Result<Vec<Observation>, String> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|error| error.to_string())?;
    let mut observations = Vec::new();
    let mut found_report = false;

    for index in 0..archive.len() {
        let mut file = archive.by_index(index).map_err(|error| error.to_string())?;
        if !file.name().ends_with(".json") {
            continue;
        }
        let mut json = String::new();
        file.read_to_string(&mut json)
            .map_err(|error| error.to_string())?;
        let Ok(report) = serde_json::from_str::<CtrfReport>(&json) else {
            continue;
        };
        found_report = true;
        for test in report.results.tests {
            let retries = match test.retries.unwrap_or(0) {
                0 if test.flaky == Some(true) => 1,
                retries => retries,
            };
            observations.push(Observation {
                name: test.name.unwrap_or_else(|| "<unnamed test>".to_string()),
                status: test
                    .status
                    .unwrap_or_else(|| "unknown".to_string())
                    .to_lowercase(),
                duration: test.duration.filter(|value| *value >= 0.0).unwrap_or(0.0),
                retries,
                run_id: context.run_id,
                attempt: context.attempt,
                branch: context.branch.clone(),
                seen_at: context.seen_at.clone(),
                artifact_name: context.artifact_name.clone(),
                run_url: context.run_url.clone(),
                job_url: None,
            });
        }
    }

    if found_report {
        Ok(observations)
    } else {
        Err("artifact contains no CTRF report".to_string())
    }
}

pub fn aggregate(mut observations: Vec<Observation>) -> Vec<TestStats> {
    observations.sort_by(|left, right| left.seen_at.cmp(&right.seen_at));
    let mut stats = HashMap::<String, TestStats>::new();
    let mut evidence = HashMap::<(String, u64), RunEvidence>::new();

    for observation in observations {
        let run = evidence
            .entry((observation.name.clone(), observation.run_id))
            .or_default();
        if observation.seen_at > run.seen_at {
            run.seen_at = observation.seen_at.clone();
        }
        run.statuses
            .entry(observation.attempt)
            .or_default()
            .insert(observation.status.clone());
        run.retries += observation.retries;
        run.main_failure |= observation.status == "failed" && observation.branch == "main";

        let test = stats
            .entry(observation.name.clone())
            .or_insert_with(|| TestStats {
                name: observation.name.clone(),
                observations: 0,
                runs: HashSet::new(),
                failure_runs: HashSet::new(),
                failures: 0,
                retries: 0,
                retried_runs: HashSet::new(),
                main_failures: 0,
                main_failure_runs: HashSet::new(),
                durations: Vec::new(),
                last_flaky: String::new(),
                clean_runs_since_flaky: 0,
                links: Vec::new(),
                flips: 0,
            });
        test.observations += 1;
        test.runs.insert(observation.run_id);
        test.durations.push(observation.duration);

        if observation.retries > 0 {
            test.retries += observation.retries;
            test.retried_runs.insert(observation.run_id);
        }
        if observation.status == "failed" {
            test.failures += 1;
            test.failure_runs.insert(observation.run_id);
            if observation.branch == "main" {
                test.main_failures += 1;
                test.main_failure_runs.insert(observation.run_id);
            }
            let link = observation.job_url.unwrap_or(observation.run_url);
            if !test.links.contains(&link) {
                test.links.push(link);
            }
        }
    }

    for ((name, _), run) in &evidence {
        let test = stats.get_mut(name).expect("test stats must exist");
        if run.is_flip() {
            test.flips += 1;
        }
        if run.is_flaky_signal() && run.seen_at > test.last_flaky {
            test.last_flaky = run.seen_at.clone();
        }
    }

    for test in stats.values_mut() {
        let mut later_runs = evidence
            .iter()
            .filter(|((name, _), run)| {
                name == &test.name && run.seen_at > test.last_flaky && run.was_executed()
            })
            .map(|(_, run)| run)
            .collect::<Vec<_>>();
        later_runs.sort_by(|left, right| left.seen_at.cmp(&right.seen_at));
        for run in later_runs {
            if run.is_clean() {
                test.clean_runs_since_flaky += 1;
            } else {
                test.clean_runs_since_flaky = 0;
            }
        }
    }

    let mut candidates = stats
        .into_values()
        .filter(|test| test.flips > 0 || test.retries > 0 || test.main_failures > 0)
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .is_active()
            .cmp(&left.is_active())
            .then_with(|| right.score().cmp(&left.score()))
            .then_with(|| {
                right
                    .fail_rate()
                    .partial_cmp(&left.fail_rate())
                    .unwrap_or(Ordering::Equal)
            })
            .then_with(|| right.last_flaky.cmp(&left.last_flaky))
            .then_with(|| right.name.cmp(&left.name))
    });
    candidates
}

pub fn report_job_name(artifact_name: &str) -> String {
    let base = strip_attempt(artifact_name)
        .strip_suffix("-report")
        .unwrap_or(strip_attempt(artifact_name));
    if base == "unit-tests" {
        "unit-tests-and-checks".to_string()
    } else if let Some(group) = base.strip_prefix("worker-executor-tests-") {
        format!("worker-tests-{group}")
    } else if base.starts_with("integration-tests-group") {
        base.to_string()
    } else if let Some(shard) = base.strip_prefix("cli-integration-tests-") {
        let shard = if shard == "bridge" {
            "bridge_gen"
        } else {
            shard
        };
        format!("it-cli ({shard})")
    } else {
        base.to_string()
    }
}

pub fn select_job_url<'a, I>(jobs: I, artifact_name: &str, fallback: &str) -> String
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let expected = report_job_name(artifact_name);
    let integration_prefix = format!("it ({expected},");
    jobs.into_iter()
        .find(|(name, _)| {
            *name == expected
                || (expected.starts_with("integration-tests-group")
                    && name.starts_with(&integration_prefix))
        })
        .map(|(_, url)| url.to_string())
        .unwrap_or_else(|| fallback.to_string())
}

pub fn attempt_from_artifact_name(name: &str) -> Option<u32> {
    name.rsplit_once("-attempt")
        .filter(|(prefix, value)| prefix.ends_with("-report") && !value.is_empty())
        .and_then(|(_, value)| value.parse().ok())
        .filter(|value| *value > 0)
}

pub fn is_report_artifact(name: &str) -> bool {
    name.ends_with("-report") || attempt_from_artifact_name(name).is_some()
}

fn strip_attempt(name: &str) -> &str {
    if attempt_from_artifact_name(name).is_some() {
        name.rsplit_once("-attempt").expect("suffix exists").0
    } else {
        name
    }
}

fn percentile(values: &[f64], fraction: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut ordered = values.to_vec();
    ordered.sort_by(f64::total_cmp);
    let rank = ((fraction * ordered.len() as f64).ceil() as usize).max(1);
    ordered[rank - 1]
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('|', "&#124;")
        .replace('\n', " ")
}

fn encode_test_name(value: &str) -> String {
    BASE64_URL_SAFE_NO_PAD.encode(value)
}

fn decode_test_name(value: &str) -> Option<String> {
    let bytes = BASE64_URL_SAFE_NO_PAD.decode(value).ok()?;
    String::from_utf8(bytes).ok()
}

fn checked_tests(body: &str) -> HashSet<String> {
    body.lines()
        .filter(|line| line.starts_with("- [x] ") || line.starts_with("- [X] "))
        .filter_map(|line| {
            let marker = line.rsplit_once("<!-- flaky-test:")?.1;
            decode_test_name(marker.strip_suffix(" -->")?)
        })
        .collect()
}

fn append_table(lines: &mut Vec<String>, tests: &[&TestStats], empty_message: &str) {
    lines.extend([
        "| Test | Score | Runs | Failures | Flips | Retries | Main failures | Fail rate | Clean runs | Last flaky | p50 | p95 | Failing jobs |".to_string(),
        "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: | --- |".to_string(),
    ]);
    if tests.is_empty() {
        lines.push(format!(
            "| {empty_message} | 0 | 0 | 0 | 0 | 0 | 0 | 0% | 0 | — | — | — | — |"
        ));
    }
    for test in tests {
        let links = if test.links.is_empty() {
            "—".to_string()
        } else {
            test.links
                .iter()
                .rev()
                .take(3)
                .rev()
                .enumerate()
                .map(|(index, url)| format!("[log {}]({url})", index + 1))
                .collect::<Vec<_>>()
                .join(" ")
        };
        lines.push(format!(
            "| <code>{}</code> | {} | {} | {} | {} | {} | {} | {:.1}% | {} | {} | {:.0} ms | {:.0} ms | {} |",
            html_escape(&test.name),
            test.score(),
            test.runs.len(),
            test.failures,
            test.flips,
            test.retries,
            test.main_failures,
            test.fail_rate() * 100.0,
            test.clean_runs_since_flaky,
            test.last_flaky.get(..10).unwrap_or(&test.last_flaky),
            percentile(&test.durations, 0.50),
            percentile(&test.durations, 0.95),
            links
        ));
    }
}

pub fn render_report(
    candidates: &[TestStats],
    run_count: usize,
    artifact_count: usize,
    days: u64,
    generated: &str,
    existing_body: &str,
    limit: usize,
) -> String {
    let shown = candidates.iter().take(limit).collect::<Vec<_>>();
    let active = shown
        .iter()
        .copied()
        .filter(|test| test.is_active())
        .collect::<Vec<_>>();
    let resolved = shown
        .iter()
        .copied()
        .filter(|test| !test.is_active())
        .collect::<Vec<_>>();
    let claimed = checked_tests(existing_body);
    let mut lines = vec![
        ISSUE_MARKER.to_string(),
        format!("# {ISSUE_TITLE}"),
        String::new(),
        format!(
            "Generated {generated} from **{run_count}** CI runs and **{artifact_count}** test-report artifacts in the last **{days} days**."
        ),
        String::new(),
        "Score = 5 × cross-attempt flips + 3 × runs with in-run retries + 2 × runs failing on `main`. Failures seen only on a feature branch are not treated as flaky unless they later pass in another attempt of the same run.".to_string(),
        String::new(),
        format!(
            "A test moves to recently resolved after **{CLEAN_RUNS_TO_RESOLVE} consecutive clean runs** following its last flaky signal. It remains visible there until that signal leaves the rolling window."
        ),
        String::new(),
        "## Active flaky tests".to_string(),
        String::new(),
    ];
    append_table(&mut lines, &active, "No active flaky tests in this window");
    lines.extend([
        String::new(),
        "## Claim an active test".to_string(),
        String::new(),
    ]);
    if active.is_empty() {
        lines.push("No tests to claim.".to_string());
    }
    for test in &active {
        let mark = if claimed.contains(&test.name) {
            "x"
        } else {
            " "
        };
        lines.push(format!(
            "- [{mark}] <code>{}</code> <!-- flaky-test:{} -->",
            html_escape(&test.name),
            encode_test_name(&test.name)
        ));
    }
    lines.extend([
        String::new(),
        "## Recently resolved".to_string(),
        String::new(),
    ]);
    append_table(
        &mut lines,
        &resolved,
        "No recently resolved flaky tests in this window",
    );
    if candidates.len() > shown.len() {
        lines.extend([
            String::new(),
            format!(
                "Showing the top {} of {} known flaky tests across both sections.",
                shown.len(),
                candidates.len()
            ),
        ]);
    }
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn observation(
        name: &str,
        status: &str,
        run_id: u64,
        attempt: u32,
        branch: &str,
        retries: u64,
        duration: f64,
    ) -> Observation {
        Observation {
            name: name.to_string(),
            status: status.to_string(),
            duration,
            retries,
            run_id,
            attempt,
            branch: branch.to_string(),
            seen_at: format!("2026-09-03T10:{run_id:02}:{attempt:02}Z"),
            artifact_name: format!("unit-tests-report-attempt{attempt}"),
            run_url: format!("https://example.test/runs/{run_id}"),
            job_url: (status == "failed")
                .then(|| format!("https://example.test/jobs/{run_id}-{attempt}")),
        }
    }

    fn report_archive(tests: serde_json::Value) -> Vec<u8> {
        let mut output = Cursor::new(Vec::new());
        {
            let mut archive = zip::ZipWriter::new(&mut output);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            archive.start_file("ctrf-report.json", options).unwrap();
            write!(
                archive,
                "{}",
                serde_json::json!({"results": {"tests": tests}})
            )
            .unwrap();
            archive.finish().unwrap();
        }
        output.into_inner()
    }

    #[test]
    fn aggregates_synthetic_ctrf_reports_across_attempts() {
        let first = report_archive(serde_json::json!([
            {"name": "suite::flip", "status": "failed", "duration": 10},
            {"name": "suite::retry", "status": "passed", "duration": 20, "retries": 2},
            {"name": "suite::flaky-flag", "status": "passed", "duration": 30, "flaky": true}
        ]));
        let second = report_archive(serde_json::json!([
            {"name": "suite::flip", "status": "passed", "duration": 30},
            {"name": "suite::retry", "status": "passed", "duration": 40}
        ]));
        let context = |attempt| ArtifactContext {
            run_id: 123,
            attempt,
            branch: "feature".to_string(),
            seen_at: format!("2026-09-03T1{attempt}:30:00Z"),
            artifact_name: format!("unit-tests-report-attempt{attempt}"),
            run_url: "https://example.test/runs/123".to_string(),
        };
        let mut observations = observations_from_archive(&first, &context(1)).unwrap();
        observations.extend(observations_from_archive(&second, &context(2)).unwrap());

        let result = aggregate(observations)
            .into_iter()
            .map(|test| (test.name.clone(), test))
            .collect::<HashMap<_, _>>();

        assert_eq!(result["suite::flip"].flips, 1);
        assert_eq!(result["suite::retry"].retries, 2);
        assert_eq!(result["suite::flaky-flag"].retries, 1);
    }

    #[test]
    fn ranks_flips_retries_and_main_failures() {
        let observations = vec![
            observation("flip", "failed", 1, 1, "feature", 0, 10.0),
            observation("flip", "passed", 1, 2, "feature", 0, 20.0),
            observation("retry", "passed", 2, 1, "feature", 2, 100.0),
            observation("main failure", "failed", 3, 1, "main", 0, 200.0),
            observation("ordinary failure", "failed", 4, 1, "feature", 0, 10.0),
        ];

        let result = aggregate(observations);

        assert_eq!(
            result
                .iter()
                .map(|test| test.name.as_str())
                .collect::<Vec<_>>(),
            vec!["flip", "retry", "main failure"]
        );
        assert_eq!(result[0].score(), 5);
        assert_eq!(result[1].score(), 3);
        assert_eq!(result[2].score(), 2);
    }

    #[test]
    fn resolves_only_after_ten_consecutive_clean_runs() {
        let mut nine_clean = vec![observation(
            "suite::flaky",
            "passed",
            1,
            1,
            "feature",
            1,
            10.0,
        )];
        nine_clean.extend(
            (2..=10).map(|run| observation("suite::flaky", "passed", run, 1, "main", 0, 10.0)),
        );

        let active = aggregate(nine_clean.clone());
        assert!(active[0].is_active());
        assert_eq!(active[0].clean_runs_since_flaky, 9);

        nine_clean.push(observation(
            "suite::flaky",
            "passed",
            11,
            1,
            "main",
            0,
            10.0,
        ));
        let resolved = aggregate(nine_clean);
        assert!(!resolved[0].is_active());
        assert_eq!(resolved[0].clean_runs_since_flaky, 10);
        assert_eq!(resolved[0].last_flaky, "2026-09-03T10:01:01Z");

        let report = render_report(&resolved, 11, 11, 30, "2026-09-03 12:00 UTC", "", 100);
        assert!(report.contains("## Recently resolved"));
        assert!(report.contains("No active flaky tests in this window"));
        assert!(!report.contains("- [ ] <code>suite::flaky</code>"));
    }

    #[test]
    fn later_non_clean_run_resets_the_clean_streak() {
        let mut observations = vec![observation(
            "suite::flaky",
            "passed",
            1,
            1,
            "feature",
            1,
            10.0,
        )];
        observations.extend(
            (2..=11).map(|run| observation("suite::flaky", "passed", run, 1, "main", 0, 10.0)),
        );
        observations.push(observation(
            "suite::flaky",
            "failed",
            12,
            1,
            "feature",
            0,
            10.0,
        ));

        let result = aggregate(observations);

        assert!(result[0].is_active());
        assert_eq!(result[0].clean_runs_since_flaky, 0);
    }

    #[test]
    fn report_limit_caps_total_tests_across_sections() {
        let mut observations = vec![observation(
            "suite::active",
            "passed",
            1,
            1,
            "feature",
            1,
            10.0,
        )];
        observations.push(observation(
            "suite::resolved",
            "passed",
            20,
            1,
            "feature",
            1,
            10.0,
        ));
        observations.extend(
            (21..=30).map(|run| observation("suite::resolved", "passed", run, 1, "main", 0, 10.0)),
        );
        let tests = aggregate(observations);

        let report = render_report(&tests, 30, 30, 30, "2026-09-03 12:00 UTC", "", 1);

        assert!(report.contains("<code>suite::active</code>"));
        assert!(!report.contains("<code>suite::resolved</code>"));
    }

    #[test]
    fn report_preserves_claimed_checkboxes() {
        let tests = aggregate(vec![observation(
            "suite::flaky",
            "passed",
            1,
            1,
            "feature",
            1,
            10.0,
        )]);
        let first = render_report(&tests, 1, 1, 30, "2026-09-03 12:00 UTC", "", 100);
        let claimed = first.replace(
            "- [ ] <code>suite::flaky</code>",
            "- [x] <code>suite::flaky</code>",
        );

        let second = render_report(&tests, 2, 2, 30, "2026-09-10 12:00 UTC", &claimed, 100);

        assert!(second.contains("- [x] <code>suite::flaky</code>"));
    }

    #[test]
    fn report_preserves_claim_from_existing_v1_issue() {
        let tests = aggregate(vec![observation(
            "suite::flaky",
            "passed",
            1,
            1,
            "feature",
            1,
            10.0,
        )]);
        let existing = concat!(
            "<!-- known-flaky-tests:v1 -->\n",
            "- [x] <code>suite::flaky</code> ",
            "<!-- flaky-test:c3VpdGU6OmZsYWt5 -->\n",
        );

        let rendered = render_report(&tests, 2, 2, 30, "2026-09-10 12:00 UTC", existing, 100);

        assert!(rendered.contains("- [x] <code>suite::flaky</code>"));
    }

    #[test]
    fn selects_exact_integration_group() {
        let jobs = [
            ("it (integration-tests-group10, IT #10)", "group10-url"),
            ("it (integration-tests-group1, IT #1)", "group1-url"),
        ];

        assert_eq!(
            select_job_url(jobs, "integration-tests-group1-report-attempt1", "fallback"),
            "group1-url"
        );
    }

    #[test]
    fn recognizes_attempt_qualified_and_historical_report_names() {
        assert!(is_report_artifact("unit-tests-report"));
        assert!(is_report_artifact("unit-tests-report-attempt3"));
        assert_eq!(
            attempt_from_artifact_name("unit-tests-report-attempt3"),
            Some(3)
        );
        assert!(!is_report_artifact("scala-sdk-integration-server-log"));
    }
}

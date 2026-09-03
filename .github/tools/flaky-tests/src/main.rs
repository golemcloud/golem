use flaky_tests::{
    ArtifactContext, ISSUE_MARKER, ISSUE_TITLE, Observation, aggregate, consolidated_artifact_name,
    observations_from_archive, render_report, select_job_url,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use std::env;
use std::error::Error;
use std::fs;
use std::io::{self, Write};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

type AnyError = Box<dyn Error + Send + Sync>;
type Result<T> = std::result::Result<T, AnyError>;

#[derive(Clone, Deserialize)]
struct WorkflowRun {
    id: u64,
    #[serde(default = "default_attempt")]
    run_attempt: u32,
    created_at: String,
    head_branch: Option<String>,
    html_url: String,
}

fn default_attempt() -> u32 {
    1
}

#[derive(Deserialize)]
struct WorkflowRuns {
    workflow_runs: Vec<WorkflowRun>,
}

#[derive(Clone, Deserialize)]
struct Artifact {
    id: u64,
    name: String,
    created_at: String,
    #[serde(default)]
    expired: bool,
    workflow_run: ArtifactWorkflowRun,
}

#[derive(Clone, Deserialize)]
struct ArtifactWorkflowRun {
    id: u64,
}

#[derive(Deserialize)]
struct Artifacts {
    artifacts: Vec<Artifact>,
}

#[derive(Deserialize)]
struct Job {
    name: String,
    html_url: String,
}

#[derive(Deserialize)]
struct Jobs {
    jobs: Vec<Job>,
}

#[derive(Clone, Deserialize)]
struct Label {
    name: String,
}

#[derive(Clone, Deserialize)]
struct Issue {
    number: u64,
    title: String,
    body: Option<String>,
    node_id: String,
    html_url: String,
    #[serde(default)]
    labels: Vec<Label>,
}

#[derive(Deserialize)]
struct Issues {
    items: Vec<Issue>,
}

#[derive(Clone)]
struct ArtifactWork {
    run: WorkflowRun,
    artifact: Artifact,
    attempt: u32,
}

struct Args {
    repo: String,
    workflow: String,
    days: u64,
    run_ids: Vec<u64>,
    limit: usize,
    output: Option<String>,
    summary: Option<String>,
    update_issue: bool,
}

struct GitHub {
    repo: String,
}

trait GitHubApi: Sync {
    fn repo(&self) -> &str;
    fn read(&self, endpoint: &str, fields: &[(&str, String)]) -> Result<Vec<u8>>;
    fn write(&self, endpoint: &str, method: &str, value: &Value) -> Result<Vec<u8>>;
    fn graphql(&self, query: &str, node_id: &str, write: bool) -> Result<Value>;

    fn read_json<T: DeserializeOwned>(
        &self,
        endpoint: &str,
        fields: &[(&str, String)],
    ) -> Result<T> {
        Ok(serde_json::from_slice(&self.read(endpoint, fields)?)?)
    }

    fn write_json<T: DeserializeOwned>(
        &self,
        endpoint: &str,
        method: &str,
        value: &Value,
    ) -> Result<T> {
        Ok(serde_json::from_slice(
            &self.write(endpoint, method, value)?,
        )?)
    }

    fn download_artifact(&self, id: u64) -> Result<Vec<u8>> {
        self.read(
            &format!("/repos/{}/actions/artifacts/{id}/zip", self.repo()),
            &[],
        )
    }

    fn paginated<T, F>(
        &self,
        endpoint: &str,
        mut extract: F,
        fields: &[(&str, String)],
    ) -> Result<Vec<T>>
    where
        T: DeserializeOwned,
        F: FnMut(Value) -> Result<Vec<T>>,
    {
        let mut result = Vec::new();
        for page in 1.. {
            let mut page_fields = fields.to_vec();
            page_fields.extend([("per_page", "100".to_string()), ("page", page.to_string())]);
            let value: Value = self.read_json(endpoint, &page_fields)?;
            let values = extract(value)?;
            let count = values.len();
            result.extend(values);
            if count < 100 {
                return Ok(result);
            }
        }
        unreachable!()
    }
}

impl GitHubApi for GitHub {
    fn repo(&self) -> &str {
        &self.repo
    }

    fn read(&self, endpoint: &str, fields: &[(&str, String)]) -> Result<Vec<u8>> {
        let mut args = vec![
            "api".to_string(),
            "--method".to_string(),
            "GET".to_string(),
            endpoint.to_string(),
        ];
        for (name, value) in fields {
            args.extend(["-f".to_string(), format!("{name}={value}")]);
        }
        command_output(&args, None, 3).map(|output| output.stdout)
    }

    fn write(&self, endpoint: &str, method: &str, value: &Value) -> Result<Vec<u8>> {
        let args = [
            "api".to_string(),
            "--method".to_string(),
            method.to_string(),
            endpoint.to_string(),
            "--input".to_string(),
            "-".to_string(),
        ];
        let input = serde_json::to_vec(value)?;
        let output = command_output(&args, Some(&input), 0)?;
        Ok(output.stdout)
    }

    fn graphql(&self, query: &str, node_id: &str, write: bool) -> Result<Value> {
        let args = [
            "api".to_string(),
            "graphql".to_string(),
            "-f".to_string(),
            format!("query={query}"),
            "-F".to_string(),
            format!("id={node_id}"),
        ];
        let output = command_output(&args, None, if write { 0 } else { 3 })?;
        let value: Value = serde_json::from_slice(&output.stdout)?;
        if let Some(errors) = value.get("errors") {
            return Err(other_error(format!("GraphQL request failed: {errors}")));
        }
        Ok(value)
    }
}

fn command_output(args: &[String], input: Option<&[u8]>, retries: u32) -> Result<Output> {
    let mut last_error = String::new();
    for attempt in 0..=retries {
        let mut child = Command::new("gh")
            .args(args)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        if let Some(input) = input {
            child
                .stdin
                .take()
                .expect("piped stdin must exist")
                .write_all(input)?;
        }
        let output = child.wait_with_output()?;
        if output.status.success() {
            return Ok(output);
        }
        last_error = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if attempt < retries {
            eprintln!(
                "GitHub request failed; retrying (attempt={}/{}, error={})",
                attempt + 1,
                retries + 1,
                last_error
            );
            thread::sleep(Duration::from_secs(1 << attempt));
        }
    }
    Err(other_error(format!(
        "gh {} failed: {last_error}",
        args.join(" ")
    )))
}

fn other_error(message: String) -> AnyError {
    Box::new(io::Error::other(message))
}

fn parse_args() -> Result<Args> {
    let mut repo = env::var("GITHUB_REPOSITORY").ok();
    let mut workflow = "ci.yaml".to_string();
    let mut days = 30;
    let mut run_ids = Vec::new();
    let mut limit = 100;
    let mut output = None;
    let mut summary = None;
    let mut update_issue = false;
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        let value = |arguments: &mut std::iter::Skip<env::Args>| {
            arguments
                .next()
                .ok_or_else(|| other_error(format!("{argument} requires a value")))
        };
        match argument.as_str() {
            "--repo" => repo = Some(value(&mut arguments)?),
            "--workflow" => workflow = value(&mut arguments)?,
            "--days" => days = value(&mut arguments)?.parse()?,
            "--run-id" => run_ids.push(value(&mut arguments)?.parse()?),
            "--limit" => limit = value(&mut arguments)?.parse()?,
            "--output" => output = Some(value(&mut arguments)?),
            "--summary" => summary = Some(value(&mut arguments)?),
            "--update-issue" => update_issue = true,
            _ => return Err(other_error(format!("unknown argument: {argument}"))),
        }
    }
    if days == 0 {
        return Err(other_error("--days must be at least 1".to_string()));
    }
    if env::var_os("GH_TOKEN").is_none() && env::var_os("GITHUB_TOKEN").is_none() {
        return Err(other_error(
            "GH_TOKEN or GITHUB_TOKEN is required".to_string(),
        ));
    }
    Ok(Args {
        repo: repo.ok_or_else(|| other_error("--repo is required".to_string()))?,
        workflow,
        days,
        run_ids,
        limit,
        output,
        summary,
        update_issue,
    })
}

fn collect_runs<G: GitHubApi>(github: &G, args: &Args, now: u64) -> Result<Vec<WorkflowRun>> {
    if !args.run_ids.is_empty() {
        return args
            .run_ids
            .iter()
            .map(|id| github.read_json(&format!("/repos/{}/actions/runs/{id}", args.repo), &[]))
            .collect();
    }

    let cutoff = now.saturating_sub(args.days * 86_400);
    let first_day = cutoff / 86_400;
    let last_day = now / 86_400;
    let mut runs = HashMap::new();
    for day in first_day..=last_day {
        let date = format_date(day as i64);
        let endpoint = format!(
            "/repos/{}/actions/workflows/{}/runs",
            args.repo, args.workflow
        );
        for page in 1.. {
            let response: WorkflowRuns = github.read_json(
                &endpoint,
                &[
                    ("created", format!("{date}T00:00:00Z..{date}T23:59:59Z")),
                    ("per_page", "100".to_string()),
                    ("page", page.to_string()),
                ],
            )?;
            let count = response.workflow_runs.len();
            for run in response.workflow_runs {
                if run.created_at >= format_timestamp(cutoff) {
                    runs.insert(run.id, run);
                }
            }
            if count < 100 {
                break;
            }
        }
    }
    Ok(runs.into_values().collect())
}

fn collect_artifacts<G: GitHubApi>(github: &G, runs: &[WorkflowRun]) -> Result<Vec<ArtifactWork>> {
    let runs_by_id = runs
        .iter()
        .map(|run| (run.id, run))
        .collect::<HashMap<_, _>>();
    let max_attempt = runs.iter().map(|run| run.run_attempt).max().unwrap_or(1);
    let mut work = Vec::new();
    let endpoint = format!("/repos/{}/actions/artifacts", github.repo());
    for attempt in 1..=max_attempt {
        let name = consolidated_artifact_name(attempt);
        let artifacts = github.paginated(
            &endpoint,
            |value| Ok(serde_json::from_value::<Artifacts>(value)?.artifacts),
            &[("name", name.clone())],
        )?;
        for artifact in artifacts {
            if artifact.expired || artifact.name != name {
                continue;
            }
            if let Some(run) = runs_by_id.get(&artifact.workflow_run.id) {
                work.push(ArtifactWork {
                    run: (*run).clone(),
                    artifact,
                    attempt,
                });
            }
        }
        eprintln!(
            "Scanned consolidated artifacts (attempt={attempt}/{max_attempt}, reports={})",
            work.len()
        );
    }
    Ok(work)
}

fn download_reports<G: GitHubApi>(github: &G, work: Vec<ArtifactWork>) -> Result<Vec<Observation>> {
    let total = work.len();
    let queue = Arc::new(Mutex::new(VecDeque::from(work)));
    let (sender, receiver) = mpsc::channel();
    thread::scope(|scope| -> Result<Vec<Observation>> {
        for _ in 0..8 {
            let queue = queue.clone();
            let sender = sender.clone();
            scope.spawn(move || {
                loop {
                    let Some(work) = queue.lock().expect("work queue is poisoned").pop_front()
                    else {
                        break;
                    };
                    let result = github
                        .download_artifact(work.artifact.id)
                        .and_then(|bytes| {
                            let context = ArtifactContext {
                                run_id: work.run.id,
                                attempt: work.attempt,
                                branch: work.run.head_branch.unwrap_or_default(),
                                seen_at: work.artifact.created_at,
                                artifact_name: work.artifact.name,
                                run_url: work.run.html_url,
                            };
                            observations_from_archive(&bytes, &context).map_err(other_error)
                        });
                    if sender.send(result).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);

        let mut observations = Vec::new();
        for (index, result) in receiver.into_iter().enumerate() {
            observations.extend(result?);
            if (index + 1) % 25 == 0 || index + 1 == total {
                eprintln!(
                    "Downloaded test reports (completed={}/{}, observations={})",
                    index + 1,
                    total,
                    observations.len()
                );
            }
        }
        Ok(observations)
    })
}

fn add_job_links<G: GitHubApi>(
    github: &G,
    observations: &mut [Observation],
    candidates: &HashSet<String>,
) -> Result<()> {
    let mut jobs_by_attempt = HashMap::<(u64, u32), Vec<Job>>::new();
    for observation in observations.iter_mut().filter(|observation| {
        observation.status == "failed" && candidates.contains(&observation.name)
    }) {
        let key = (observation.run_id, observation.attempt);
        if let std::collections::hash_map::Entry::Vacant(entry) = jobs_by_attempt.entry(key) {
            let endpoint = format!(
                "/repos/{}/actions/runs/{}/attempts/{}/jobs",
                github.repo(),
                observation.run_id,
                observation.attempt
            );
            let jobs = github.paginated(
                &endpoint,
                |value| Ok(serde_json::from_value::<Jobs>(value)?.jobs),
                &[],
            )?;
            entry.insert(jobs);
        }
        let fallback = format!("{}/attempts/{}", observation.run_url, observation.attempt);
        observation.job_url = Some(select_job_url(
            jobs_by_attempt[&key]
                .iter()
                .map(|job| (job.name.as_str(), job.html_url.as_str())),
            &observation.artifact_name,
            &fallback,
        ));
    }
    Ok(())
}

fn find_issue<G: GitHubApi>(github: &G) -> Result<Option<Issue>> {
    let response: Issues = github.read_json(
        "/search/issues",
        &[
            (
                "q",
                format!("repo:{} is:issue in:title \"{ISSUE_TITLE}\"", github.repo()),
            ),
            ("per_page", "100".to_string()),
        ],
    )?;
    let mut exact = None;
    for issue in response.items {
        if issue.title != ISSUE_TITLE {
            continue;
        }
        if issue
            .body
            .as_deref()
            .unwrap_or_default()
            .contains(ISSUE_MARKER)
        {
            return Ok(Some(issue));
        }
        exact.get_or_insert(issue);
    }
    Ok(exact)
}

fn ensure_label<G: GitHubApi>(github: &G) -> Result<()> {
    let endpoint = format!("/repos/{}/labels/flaky-test", github.repo());
    let output = github.read(&endpoint, &[]);
    match output {
        Ok(_) => Ok(()),
        Err(error) if error.to_string().contains("HTTP 404") => {
            let _: Value = github.write_json(
                &format!("/repos/{}/labels", github.repo()),
                "POST",
                &json!({
                    "name": "flaky-test",
                    "color": "d73a4a",
                    "description": "Tests with evidence of intermittent failure"
                }),
            )?;
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn pin_issue<G: GitHubApi>(github: &G, issue: &Issue) -> Result<()> {
    let query = "query($id:ID!){node(id:$id){... on Issue{isPinned}}}";
    let value = github.graphql(query, &issue.node_id, false)?;
    if value
        .pointer("/data/node/isPinned")
        .and_then(Value::as_bool)
        == Some(true)
    {
        return Ok(());
    }
    let mutation = "mutation($id:ID!){pinIssue(input:{issueId:$id}){issue{id}}}";
    github.graphql(mutation, &issue.node_id, true)?;
    Ok(())
}

fn upsert_issue<G: GitHubApi>(github: &G, existing: Option<Issue>, body: &str) -> Result<String> {
    ensure_label(github)?;
    let issue: Issue = if let Some(existing) = existing {
        let mut labels = existing
            .labels
            .iter()
            .map(|label| label.name.clone())
            .collect::<HashSet<_>>();
        labels.insert("flaky-test".to_string());
        github.write_json(
            &format!("/repos/{}/issues/{}", github.repo(), existing.number),
            "PATCH",
            &json!({
                "title": ISSUE_TITLE,
                "body": body,
                "labels": labels,
                "state": "open"
            }),
        )?
    } else {
        github.write_json(
            &format!("/repos/{}/issues", github.repo()),
            "POST",
            &json!({"title": ISSUE_TITLE, "body": body, "labels": ["flaky-test"]}),
        )?
    };
    pin_issue(github, &issue)?;
    Ok(issue.html_url)
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

fn format_date(days: i64) -> String {
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

fn format_timestamp(seconds: u64) -> String {
    let date = format_date((seconds / 86_400) as i64);
    let seconds = seconds % 86_400;
    format!(
        "{date}T{:02}:{:02}:{:02}Z",
        seconds / 3_600,
        seconds % 3_600 / 60,
        seconds % 60
    )
}

fn generate_report<G: GitHubApi>(
    github: &G,
    args: &Args,
    now: u64,
) -> Result<(String, Option<Issue>)> {
    eprintln!(
        "Discovering CI runs (repository={}, workflow={}, window_days={})",
        args.repo, args.workflow, args.days
    );
    let runs = collect_runs(github, args, now)?;
    eprintln!("Discovered CI runs (runs={})", runs.len());
    eprintln!("Discovering test-report artifacts");
    let work = collect_artifacts(github, &runs)?;
    let artifact_count = work.len();
    let analyzed_run_count = work
        .iter()
        .map(|work| work.run.id)
        .collect::<HashSet<_>>()
        .len();
    eprintln!("Discovered test-report artifacts (reports={artifact_count})");
    eprintln!("Downloading and parsing test reports (workers=8)");
    let mut observations = download_reports(github, work)?;
    eprintln!("Parsed test reports (observations={})", observations.len());
    let candidate_names = aggregate(observations.clone())
        .into_iter()
        .map(|test| test.name)
        .collect::<HashSet<_>>();
    eprintln!(
        "Resolving failing job links (candidate_tests={})",
        candidate_names.len()
    );
    add_job_links(github, &mut observations, &candidate_names)?;
    eprintln!("Aggregating flaky-test signals");
    let candidates = aggregate(observations);
    let active_count = candidates.iter().filter(|test| test.is_active()).count();
    eprintln!(
        "Aggregated flaky-test signals (active={}, recently_resolved={})",
        active_count,
        candidates.len() - active_count
    );
    let existing = if args.update_issue {
        eprintln!("Looking up the known-flaky-tests issue");
        find_issue(github)?
    } else {
        None
    };
    let generated = format_timestamp(now).replace('T', " ").replace('Z', " UTC");
    let body = render_report(
        &candidates,
        analyzed_run_count,
        runs.len(),
        artifact_count,
        args.days,
        &generated,
        existing
            .as_ref()
            .and_then(|issue| issue.body.as_deref())
            .unwrap_or_default(),
        args.limit,
    );
    Ok((body, existing))
}

fn main() -> Result<()> {
    let args = parse_args()?;
    let github = GitHub {
        repo: args.repo.clone(),
    };
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let (body, existing) = generate_report(&github, &args, now)?;
    if let Some(output) = &args.output {
        fs::write(output, &body)?;
    } else {
        print!("{body}");
    }
    if let Some(summary) = &args.summary {
        fs::write(summary, &body)?;
    }
    if args.update_issue {
        eprintln!("Updating and pinning the known-flaky-tests issue");
        eprintln!("Updated {}", upsert_issue(&github, existing, &body)?);
    }
    eprintln!("Flaky-test report completed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    struct FakeGitHub {
        reads: Mutex<Vec<String>>,
        writes: Mutex<Vec<(String, String)>>,
        graphql_writes: Mutex<usize>,
        first_archive: Vec<u8>,
        second_archive: Vec<u8>,
        existing_issue: bool,
        label_exists: bool,
        pinned: bool,
    }

    impl FakeGitHub {
        fn new(existing_issue: bool, label_exists: bool, pinned: bool) -> Self {
            Self {
                reads: Mutex::new(Vec::new()),
                writes: Mutex::new(Vec::new()),
                graphql_writes: Mutex::new(0),
                first_archive: report_archive(
                    "unit-tests-report-attempt1",
                    json!([
                        {"name": "suite::flip", "status": "failed", "duration": 10},
                        {"name": "suite::retry", "status": "passed", "duration": 20, "retries": 2}
                    ]),
                ),
                second_archive: report_archive(
                    "unit-tests-report-attempt2",
                    json!([
                        {"name": "suite::flip", "status": "passed", "duration": 30},
                        {"name": "suite::retry", "status": "passed", "duration": 40}
                    ]),
                ),
                existing_issue,
                label_exists,
                pinned,
            }
        }

        fn issue_json() -> Value {
            json!({
                "number": 42,
                "title": ISSUE_TITLE,
                "body": ISSUE_MARKER,
                "node_id": "issue-node",
                "html_url": "https://example.test/issues/42",
                "labels": [{"name": "triage"}]
            })
        }
    }

    impl GitHubApi for FakeGitHub {
        fn repo(&self) -> &str {
            "test/repo"
        }

        fn read(&self, endpoint: &str, fields: &[(&str, String)]) -> Result<Vec<u8>> {
            self.reads.lock().unwrap().push(endpoint.to_string());
            let value = match endpoint {
                "/repos/test/repo/actions/runs/1" => json!({
                    "id": 1,
                    "run_attempt": 2,
                    "created_at": "2026-09-03T10:00:00Z",
                    "head_branch": "main",
                    "html_url": "https://example.test/runs/1"
                }),
                "/repos/test/repo/actions/artifacts" => {
                    let name = fields
                        .iter()
                        .find(|(key, _)| *key == "name")
                        .map(|(_, value)| value.as_str())
                        .unwrap();
                    let (id, created_at) = match name {
                        "ci-test-reports-attempt1" => (11, "2026-09-03T10:30:00Z"),
                        "ci-test-reports-attempt2" => (12, "2026-09-03T11:30:00Z"),
                        _ => return Err(other_error(format!("unexpected artifact name {name}"))),
                    };
                    json!({"artifacts": [{
                        "id": id,
                        "name": name,
                        "created_at": created_at,
                        "expired": false,
                        "workflow_run": {"id": 1}
                    }]})
                }
                "/repos/test/repo/actions/artifacts/11/zip" => {
                    return Ok(self.first_archive.clone());
                }
                "/repos/test/repo/actions/artifacts/12/zip" => {
                    return Ok(self.second_archive.clone());
                }
                "/repos/test/repo/actions/runs/1/attempts/1/jobs"
                | "/repos/test/repo/actions/runs/1/attempts/2/jobs" => json!({"jobs": [{
                    "name": "unit-tests-and-checks",
                    "html_url": "https://example.test/jobs/unit-tests"
                }]}),
                "/search/issues" => json!({
                    "items": if self.existing_issue { vec![Self::issue_json()] } else { vec![] }
                }),
                "/repos/test/repo/labels/flaky-test" if self.label_exists => {
                    json!({"name": "flaky-test"})
                }
                "/repos/test/repo/labels/flaky-test" => {
                    return Err(other_error("gh failed (HTTP 404)".to_string()));
                }
                _ => return Err(other_error(format!("unexpected read: {endpoint}"))),
            };
            Ok(serde_json::to_vec(&value)?)
        }

        fn write(&self, endpoint: &str, method: &str, _value: &Value) -> Result<Vec<u8>> {
            self.writes
                .lock()
                .unwrap()
                .push((method.to_string(), endpoint.to_string()));
            let value = if endpoint.ends_with("/labels") {
                json!({})
            } else {
                Self::issue_json()
            };
            Ok(serde_json::to_vec(&value)?)
        }

        fn graphql(&self, _query: &str, _node_id: &str, write: bool) -> Result<Value> {
            if write {
                *self.graphql_writes.lock().unwrap() += 1;
                Ok(json!({"data": {"pinIssue": {"issue": {"id": "issue-node"}}}}))
            } else {
                Ok(json!({"data": {"node": {"isPinned": self.pinned}}}))
            }
        }
    }

    fn report_archive(artifact_name: &str, tests: Value) -> Vec<u8> {
        let mut output = Cursor::new(Vec::new());
        {
            let mut archive = zip::ZipWriter::new(&mut output);
            archive
                .start_file(
                    format!("{artifact_name}/ctrf-report.json"),
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
            write!(archive, "{}", json!({"results": {"tests": tests}})).unwrap();
            archive.finish().unwrap();
        }
        output.into_inner()
    }

    fn test_args(update_issue: bool) -> Args {
        Args {
            repo: "test/repo".to_string(),
            workflow: "ci.yaml".to_string(),
            days: 30,
            run_ids: vec![1],
            limit: 100,
            output: None,
            summary: None,
            update_issue,
        }
    }

    #[test]
    fn formats_unix_dates() {
        assert_eq!(format_timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_timestamp(1_788_480_000), "2026-09-04T00:00:00Z");
    }

    #[test]
    fn exercises_collection_aggregation_and_issue_creation_without_processes() {
        let github = FakeGitHub::new(false, false, false);

        let (body, existing) = generate_report(&github, &test_args(true), 1_788_480_000).unwrap();

        assert!(existing.is_none());
        assert!(body.contains("**1 of 1** CI runs"));
        assert!(body.contains("<code>suite::flip</code> | 7"));
        assert!(body.contains("<code>suite::retry</code> | 3"));
        assert!(body.contains("[log 1](https://example.test/jobs/unit-tests)"));
        assert_eq!(
            github
                .reads
                .lock()
                .unwrap()
                .iter()
                .filter(|endpoint| endpoint.as_str() == "/repos/test/repo/actions/artifacts")
                .count(),
            2
        );
        assert_eq!(
            upsert_issue(&github, existing, &body).unwrap(),
            "https://example.test/issues/42"
        );
        assert_eq!(
            github.writes.lock().unwrap().as_slice(),
            &[
                ("POST".to_string(), "/repos/test/repo/labels".to_string()),
                ("POST".to_string(), "/repos/test/repo/issues".to_string())
            ]
        );
        assert_eq!(*github.graphql_writes.lock().unwrap(), 1);
    }

    #[test]
    fn updates_an_existing_labelled_and_pinned_issue() {
        let github = FakeGitHub::new(true, true, true);
        let (body, existing) = generate_report(&github, &test_args(true), 1_788_480_000).unwrap();

        upsert_issue(&github, existing, &body).unwrap();

        assert_eq!(
            github.writes.lock().unwrap().as_slice(),
            &[(
                "PATCH".to_string(),
                "/repos/test/repo/issues/42".to_string()
            )]
        );
        assert_eq!(*github.graphql_writes.lock().unwrap(), 0);
    }
}

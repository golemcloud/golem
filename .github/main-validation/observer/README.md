# Main validation observer setup

`PROMPT.md` owns the reusable observer instructions. All required source is in this directory: observation helpers, report publishers, and the dashboard's HTML, server, tests and Box deployment configuration in `dashboard/`. Only generated evidence, history and delivery receipts live outside Git. Nothing depends on an orb-backup script or thread history to recreate the observer.

The observer never builds or executes the application under investigation. It only reads CI evidence, source and commits, then publishes the dashboard and Slack reports. The dashboard is a read model, not a job runner or CI controller.

## Start an observer

1. Use a dedicated checkout of an approved Golem revision on a small persistent orb or another host with Git, `gh`, Bash, jq and Python 3. No Golem build toolchain is required. Run from the checkout root. Git history must include the baseline and source commits used for analysis; fetch missing history when needed.
2. Configure GitHub read access to repository contents, Actions runs, logs and artifacts. Configure `SLACK_BENCHMARK_WEBHOOK_URL` and `CI_DASHBOARD_TOKEN` as secrets, and `CI_DASHBOARD_URL` as the dashboard's public HTTPS origin. The current Slack destination is `#golem-dev-internal`; the webhook, not the prompt, selects the channel. Provision the same dashboard token on the Box deployment. Do not put credentials in Git or a thread prompt.
3. Generated state defaults to the Git-ignored `.local/ci-observer/` directory. Set `OBSERVER_STATE_DIR` to an absolute durable path to store it elsewhere. `seen/` records completed processing, `receipts/` prevents duplicate Slack posts, and `runs/` contains collected evidence and analysis. Only one observer may own that state and report stream at a time.
4. Start a thread in this checkout and ask it to read and follow `PROMPT.md`. In Amp, configure an `existing`-mode schedule with `RRULE:FREQ=MINUTELY;INTERVAL=30`. Use the short wake-up prompt below rather than copying the full instructions into the schedule. Another scheduler can use the same file and state directory; it must prevent concurrent invocations.
5. Verify read access, secret presence without printing it, dashboard publication, and the saved schedule by reading it back. Accept reporting on one real failed and one green Main validation run and a repeated wake-up that does not duplicate a Slack post. Do not manufacture production failures or use ordinary CI as acceptance. Building and testing these reporting tools is the operator's development task, not part of observer wake-ups.

### Wake-up prompt

```text
Read .github/main-validation/observer/PROMPT.md in the configured Golem checkout and follow it for this wake-up. Use the configured OBSERVER_STATE_DIR, or the default documented there. Do not create another schedule or observer.
```

The operator supplies authorization to post and, during rollout, an optional implementation-thread destination for acceptance evidence. The reusable prompt does not encode a particular Amp account, thread ID or absolute checkout path.

## Iterate or move accounts

Edit `PROMPT.md` and review the diff. Once approved, update the observer checkout to that revision before its next wake-up. The schedule reads the file each time; it does not fetch or deploy prompt changes automatically. A prompt edit changes future behavior only after that checkout is updated.

To replace an observer, pause the old schedule and wait for its current invocation to finish. Transfer the complete state directory, especially `seen/` and `receipts/`, before enabling the replacement. If the earlier deployment used `.amp/in/orb-backup/gol-1971-observer/`, move its generated state to the new location or explicitly point `OBSERVER_STATE_DIR` at it; none of its old helper scripts is needed. Starting with empty state can replay old failures and duplicate Slack posts. Preserve uncertain receipts for the operator to reconcile. Provision credentials separately; never copy secret values through Git or thread messages.

## Always-available dashboard

The public UI is static HTML with no frontend build or dependencies. A small Python server serves it and a public `/data.json` read model. Only authenticated observer requests can update `/api/run` and `/api/heartbeat`. The server cannot dispatch workflows, access GitHub, run Golem or analyze code. It has no GitHub or Slack credentials.

Deploy `dashboard/` to an always-on Amp Box with its `.amp/box.yaml`. The operator commits and pushes the source, then runs `amp boxes deploy --on <namespace>/<box> --name <deployment> --branch <branch> --public` from that folder. Provision `CI_DASHBOARD_TOKEN` as a deployment secret, then redeploy. The page works without a token, but publishing fails closed until it is configured. Use the returned deployment URL as `CI_DASHBOARD_URL`; deployment targeting is saved in the root `.amp/box-deploy.json`.

`AMP_DEPLOYMENT_DATA_DIRECTORY/data.json` stores the latest 200 run attempts and survives service restarts and code redeploys. Back it up before moving or deleting the Box. To use another host, set `DASHBOARD_DATA_DIR`, `CI_DASHBOARD_TOKEN` and `PORT`, then serve `python3 server.py` behind HTTPS. The observer only publishes data; it does not deploy the dashboard each wake-up.

The page refreshes every 30 seconds, while observations arrive on the 30-minute schedule. A timestamp older than 45 minutes shows a stale warning. Publishing a heartbeat records a successful observation without inventing a run. Public data contains selected fields and sanitized failure snippets, never raw log archives or secrets. The Box must remain on for the public URL to remain available.

For development, run `python3 -m unittest discover -s .github/main-validation/observer/dashboard -p 'test_*.py'`. These are reporting-tool tests, not observer instructions or Golem workload tests.

## Helpers and limits

- `list-runs.sh` lists unseen completed attempts; `--active` lists current activity. Discovery reads the latest 100 runs for each event; a longer outage needs a deliberate backfill.
- `run-facts.sh <run> <attempt>` downloads job facts, logs and raw Linux/APFS reports into `runs/<run>-a<attempt>/`. Missing logs or expired reports can limit analysis. Re-running it replaces that attempt's evidence directory, not its receipt or seen marker.
- `ctrf-summary.py <artifact-directory>` extracts failed and recovered tests without executing artifact content.
- `post-slack.py <run> <attempt> <kind> <message-file>` creates an exclusive receipt before posting. `pending`, `unknown` and `rejected` receipts require a human decision; only `confirmed` means delivery was verified.
- `publish-dashboard.py <facts.json> <analysis.json>` upserts a run attempt and returns its public permalink. `--heartbeat` updates observation freshness without creating a run. Dashboard retries are idempotent; Slack retries are not.

Workflow reruns replace the raw artifacts under the same run ID. The attempt metadata and jobs are attempt-specific, but those raw reports are not guaranteed to describe an older attempt. State that limitation when analyzing an earlier attempt after a rerun; do not claim the test evidence is attempt-specific unless verified.

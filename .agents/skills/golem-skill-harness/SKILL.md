---
name: golem-skill-harness
description: "Developing, testing, and running Golem skill scenarios with the TypeScript skill harness. Use when creating Golem skills, authoring scenario YAML, running agent matrices, or debugging skill activation and behavior failures."
---

# Golem Skill Test Harness

The harness in `golem-skills/tests/harness/` runs coding agents against current-schema YAML scenarios. Treat `src/executor.ts`, `src/assertions.ts`, `src/driver/`, `src/workspace.ts`, and `src/services.ts` as the source of truth; do not add legacy fields or compatibility shims to new scenarios.

## Skills and generated documentation

Catalog skills live in `golem-skills/skills/{common,rust,ts,scala,moonbit}/<skill>/SKILL.md`. Golem templates install common plus language-specific skills in the canonical `.agents/skills/` tree. Harness drivers also seed bootstrap skills there; Claude receives a `.claude -> .agents` symlink rather than a second catalog copy.

After changing catalog skills:

```shell
cargo build -p golem
cargo make generate-docs-skills
```

The CLI embeds the catalog. The docs command updates the generated How-To index (`docs/src/content/next/how-to-guides.mdx`), category landing pages, `_meta.js` files, and individual guide pages. If `target/release/golem` exists, the harness chooses it before `target/debug/golem`; rebuild the selected profile or remove the stale binary.

## Prerequisites and limitations

- Node.js/npm and one requested agent runtime: Amp SDK credentials, `claude`, `opencode`, `codex`, or `gemini`.
- `GOLEM_PATH` must identify the workspace root. If absent, detection walks upward from the **process current directory** looking for both `sdks/rust/golem-rust` and `sdks/ts/packages`.
- A built executable must exist at literal `<GOLEM_PATH>/target/release/golem` or `<GOLEM_PATH>/target/debug/golem`. `resolveGolemTargetDir` does not honor `CARGO_TARGET_DIR` or Cargo target-dir configuration.
- The harness prepends the selected target directory to `PATH`, starts its own clean Golem server, and requires port **9881** to be free. The runner always starts on 9881. Although the scenario schema exposes `settings.golem_server.router_port`, changing it does not change server startup and is therefore not a usable alternate-port mechanism.
- Docker is required for scenario prerequisite services (`postgres`, `mysql`, `ignite`, `openai-mock`).
- Linux skill activation fallback uses `inotifywait` when available. macOS deliberately starts no `fswatch` process. Both platforms can use atime snapshots; filesystems or mounts that do not update atime reliably can miss reads. Amp, Claude, OpenCode, and Gemini primarily report native skill-tool events; Codex falls back to filesystem tracking.
- Language toolchains must match the scenario. Rust commonly needs `wasm32-wasip2`; TypeScript needs pnpm and built SDK/template artifacts; MoonBit needs `moon` and `wasm-tools`.
- Scala runs need Java 17, sbt, synchronized WIT, the three generated guest-runtime role WASMs
  under `sdks/scala/{sbt/src/main/resources,mill/resources}/golem/wasm/`, and the Scala SDK
  published locally for the versions used by templates. The Scala plugin/template build combines
  those resources into its application-local `.generated/agent_guest.wasm`; there is no generated
  `sdks/scala/agent_guest.wasm`. Follow `.github/actions/run-skill-harness/action.yml` for the
  authoritative CI setup (including the TS SDK artifacts Scala currently consumes).

## Install and fast checks

```shell
cd golem-skills/tests/harness
npm install
npm run build          # lint + tsc
npm run format:check
npm test               # tests compiled into dist first
```

Repository policy forbids unit tests from spawning external tools or compilers. Keep schema, assertion, watcher, and driver parsing logic in harness unit tests; validate real agent CLIs, Golem, Docker services, and language compilers through harness scenarios/CI.

## Run scenarios

All relative CLI paths are resolved from the process current directory, not from the location of `run.ts`. Normally run from `golem-skills/tests/harness/`:

```shell
npx tsx src/run.ts --dry-run --scenario <name>
npx tsx src/run.ts --agent claude-code --language rust --scenario <name>
```

Current options:

| Option                      | Meaning                                                                     | Default                     |
| --------------------------- | --------------------------------------------------------------------------- | --------------------------- |
| `--agent <name>`            | `amp`, `claude-code`, `opencode`, `codex`, or `gemini`; `all` runs all      | `all`                       |
| `--language <lang>`         | `ts`, `rust`, `scala`, or `moonbit`; `all` runs all                         | `all`                       |
| `--scenario <name>`         | Select one scenario                                                         | all                         |
| `--model <id>`              | Model selection (currently sets `OPENCODE_MODEL`; also recorded in reports) | agent default               |
| `--scenarios <dir>`         | Scenario directory, relative to cwd                                         | `./scenarios`               |
| `--output <dir>`            | Results directory, relative to cwd                                          | `./results`                 |
| `--timeout <seconds>`       | Global step timeout                                                         | 1800                        |
| `--idle-timeout <seconds>`  | No-output timeout                                                           | 300; Gemini defaults to 600 |
| `--retries <n>`             | Whole-scenario retries after idle timeout                                   | 5                           |
| `--dry-run`                 | Parse/validate and summarize only                                           | false                       |
| `--resume-from <id>`        | Start at a step ID; requires `--scenario`                                   | unset                       |
| `--workspace <path>`        | Root under which the run UUID hierarchy is created                          | `./workspaces`              |
| `--merge-reports <dir>`     | Merge discovered summaries/reports                                          | unset                       |
| `--regenerate-report <url>` | Rebuild HTML from published report data; `latest` uses the built-in URL     | unset                       |
| `--ctrf <path>`             | Write/update a CTRF report                                                  | unset                       |

`--workspace` is a root override, not an exact reusable scenario directory. Workspaces are `<root>/<run-uuid>/<scenario>/<language>/` (retries add subdirectories) and are retained. `--resume-from` still creates a new run hierarchy; it skips earlier scenario steps rather than reopening an old workspace.

The server is restarted between scenarios and retries with the same clean data directory. The harness refuses a pre-existing healthy server on 9881.

## Current scenario schema

```yaml
name: example # required
languageAgnostic: false # optional; once per agent on first selected language
settings:
  timeout_per_subprompt: 1800
  golem_server:
    router_port: 9881 # keep 9881; startup is fixed there
    custom_request_port: 9006 # exported to scenario commands
  cleanup: true # accepted; unique workspaces make cleanup unnecessary
prerequisites:
  env: { EXTRA_VAR: "value" }
  services: [postgres] # postgres|mysql|ignite|openai-mock
skip_if: { agent: codex, language: ts, os: linux }
steps:
  - id: create
    create_project: { name: test-app }
    verify:
      build: true
      deploy: true
      expectedFiles: [test-app/golem.yaml]
finally: [] # same step schema; best-effort cleanup
```

Every step has exactly one action:

- `prompt`: string or language map. Supports `expectedSkills`, `allowedExtraSkills`, `strictSkillMatch`, and `continueSession`.
- `create_project`: `{ name, presets? }`; presets may be language-specific.
- `shell`: `{ command, args?, cwd? }`, with args optionally language-specific. It executes directly, not through a shell.
- `invoke` / `invoke_json`: `{ agent, method, args? }`; method and args may be language-specific.
- `trigger`: fire-and-forget form of invocation.
- `create_agent`: `{ name, env?, config? }`; `delete_agent`: `{ name }`.
- `http`: `{ url, method?, headers?, body? }`; methods include GET, POST, PUT, DELETE, PATCH, OPTIONS.
- `get_agent_type`: `{ name }`; `list_agent_types`: `{}`.
- `check_file`: `{ path }`, resolved from the discovered Golem app directory.
- `mcp_call`: `{ url, method, params? }`, using Streamable HTTP session initialization.
- `sleep`: seconds.

Common optional step fields are `id`, `timeout`, `expect`, `retry: { attempts, delay }`, `only_if`, `skip_if`, and `verify: { build?, deploy?, expectedFiles? }`. `allowedExtraSkills` or `strictSkillMatch` requires `expectedSkills`. Without an explicit extra-skill restriction, additional activations do not fail the step.

Language-conditional values use `{ ts, rust, scala, moonbit }` maps. Supported conditional fields include prompts, skill lists, `verify`, `expect`, project presets, shell args, HTTP body, and invocation/trigger method and args. A missing selected-language entry resolves to absent and may fail later validation/execution; include every language the scenario runs.

Use source-language method names: snake_case for Rust/MoonBit and camelCase for TypeScript/Scala. Use language-specific argument syntax where composite values differ.

## Assertions

`expect` supports:

- process/file output: `exit_code`, `stdout_contains`, `stdout_not_contains`, `stdout_matches`
- HTTP/MCP: `status`, `body_contains`, `body_matches`, `header_contains`
- JSON bodies: `body_json`
- unwrapped `invoke_json` results: `result_json`

`body_json` and `result_json` are arrays of `{ path, equals?, equals_unordered?, contains? }`. `equals_unordered` compares top-level arrays without order. Regexes are JavaScript `RegExp`, validated by dry-run; avoid PCRE-only syntax.

## Variables and paths

Text substitution recognizes `{{workspace}}`, `{{scenario}}`, `{{agent}}`, `{{language}}`, plus service variables `{{postgres_url}}`, `{{mysql_url}}`, `{{ignite_url}}`, and `{{openai_mock_url}}` when started. Substitution applies only to fields implemented in `substituteStepVariables`; do not assume arbitrary YAML strings are expanded.

`shell.cwd` is relative to the scenario workspace. Most Golem actions discover `golem.yaml` at the workspace root or one immediate child and run from that app directory. `check_file.path` is app-relative. `expectedFiles` verification is workspace-relative, matching existing scenarios (which normally include the project directory).

## Skill activation and sessions

Native driver events are preferred when available; otherwise watcher events and changed atimes are combined. Activations accumulate across follow-ups. The first prompt and any `continueSession: false` prompt reset the activation session. Because activation detection proves that a skill was opened/reported, not that its advice was followed, pair `expectedSkills` with build, deploy, file, invocation, HTTP, or JSON assertions.

## Reports and CI

Normal runs write per-scenario JSON (`<agent>-<language>-<scenario>.json`), `summary.json`, and `report.html`; `--ctrf` adds CTRF JSON, and `GITHUB_STEP_SUMMARY` receives a summary. Merge mode writes `merged-summary.json`, `scenario-reports.json`, and `report.html`.

- `.github/workflows/ci.yaml` runs harness build, formatting, and unit tests when `golem-skills/**` changes.
- `.github/workflows/skill-harness.yaml` is manually dispatched (scheduled runs are disabled), runs provider/language matrices, merges artifacts, and publishes the report.
- `.github/actions/run-skill-harness/action.yml` defines integration prerequisites and executes the harness from an isolated `/tmp/harness-run` while `GOLEM_PATH` points at the checkout.

## Debugging workflow

1. Run `--dry-run --scenario <name>` to catch current-schema and regex errors.
2. Run one agent/language/scenario with explicit timeouts; inspect the retained workspace and per-step JSON.
3. Distinguish step timeout from idle timeout. Whole-scenario retry occurs only for idle timeout; per-step `retry` handles any step failure.
4. For missed activation, check native tool events first, then `.agents/skills`, atime behavior, and Linux `inotifywait`. Do not restore stale `.claude/skills` duplication.
5. For stale skill behavior, confirm which release/debug `golem` was selected and rebuild it.
6. For Golem failures, verify port 9881 is free and inspect server output. For service failures, inspect Docker availability/container logs.
7. For Scala, reproduce the composite action's Java/sbt/WIT/base-image/local-publish preparation before blaming the scenario.

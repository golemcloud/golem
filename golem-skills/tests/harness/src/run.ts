import { parseArgs } from "node:util";
import * as path from "node:path";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import { randomUUID } from "node:crypto";
import { ReportBuilder, TestBuilder, stringify } from "ctrf";
import {
  ScenarioLoader,
  ScenarioExecutor,
  DEFAULT_STEP_TIMEOUT_SECONDS,
  DEFAULT_IDLE_TIMEOUT_SECONDS,
  type ScenarioRunResult,
} from "./executor.js";
import { AmpAgentDriver } from "./driver/amp.js";
import { ClaudeAgentDriver } from "./driver/claude.js";
import { OpenCodeAgentDriver } from "./driver/opencode.js";
import { CodexAgentDriver } from "./driver/codex.js";
import { GeminiAgentDriver } from "./driver/gemini.js";
import type { AgentDriver } from "./driver/base.js";
import { SkillWatcher } from "./watcher.js";
import {
  generateHtmlReport,
  type Summary,
  type MergedSummary,
  type HtmlScenarioReport,
} from "./html-report.js";
import * as log from "./log.js";
import { formatScenarioMatrixLabel, renderGitHubStepSummary } from "./summary.js";
import { detectGolemWorkspaceRoot, resolveGolemTargetDir, GolemServer } from "./workspace.js";

const DEFAULT_SCENARIO_RETRIES = 5;

const SUPPORTED_AGENTS = ["amp", "claude-code", "opencode", "codex", "gemini"] as const;
const SUPPORTED_LANGUAGES = ["ts", "rust", "scala"] as const;

type SupportedAgent = (typeof SUPPORTED_AGENTS)[number];
type SupportedLanguage = (typeof SUPPORTED_LANGUAGES)[number];

interface ScenarioReport {
  scenario: string;
  matrix: { agent: string; language: string };
  run_id: string;
  status: "pass" | "fail";
  durationSeconds: number;
  results: ScenarioRunResult["stepResults"];
  artifactPaths: string[];
}

function createDriver(agent: SupportedAgent): AgentDriver {
  switch (agent) {
    case "amp":
      return new AmpAgentDriver();
    case "claude-code":
      return new ClaudeAgentDriver();
    case "opencode":
      return new OpenCodeAgentDriver();
    case "codex":
      return new CodexAgentDriver();
    case "gemini":
      return new GeminiAgentDriver();
  }
}

async function mergeReports(reportsDir: string, outputDir: string): Promise<void> {
  await fs.mkdir(outputDir, { recursive: true });

  const files: string[] = [];
  const topLevel = (await fs.readdir(reportsDir)).filter(
    (f) => f === "summary.json" || f.endsWith("-summary.json"),
  );
  if (topLevel.length > 0) {
    files.push(...topLevel);
  } else {
    // Try reading summary.json from subdirectories
    const entries = await fs.readdir(reportsDir, { withFileTypes: true });
    for (const entry of entries) {
      if (entry.isDirectory()) {
        try {
          const summaryPath = path.join(reportsDir, entry.name, "summary.json");
          await fs.access(summaryPath);
          files.push(path.join(entry.name, "summary.json"));
        } catch {
          // no summary in this dir
        }
      }
    }
  }

  const summaries: Summary[] = [];
  for (const file of files) {
    const content = await fs.readFile(path.join(reportsDir, file), "utf8");
    summaries.push(JSON.parse(content) as Summary);
  }

  if (summaries.length === 0) {
    log.error("No summary.json files found in the reports directory");
    process.exit(1);
  }

  const agents = new Set<string>();
  const languages = new Set<string>();
  const osSet = new Set<string>();
  const heatMap: MergedSummary["heatMap"] = [];

  for (const s of summaries) {
    const agent = s.agent ?? "unknown";
    const lang = s.language ?? "unknown";
    const sOs = s.os ?? "unknown";
    agents.add(agent);
    languages.add(lang);
    osSet.add(sOs);
    heatMap.push({
      agent,
      language: lang,
      os: sOs,
      total: s.total,
      passed: s.passed,
      failed: s.failed,
    });
  }

  const merged: MergedSummary = {
    overallTotal: summaries.reduce((sum, s) => sum + s.total, 0),
    overallPassed: summaries.reduce((sum, s) => sum + s.passed, 0),
    overallFailed: summaries.reduce((sum, s) => sum + s.failed, 0),
    matrix: {
      agents: Array.from(agents),
      languages: Array.from(languages),
      os: Array.from(osSet),
    },
    heatMap,
    summaries,
  };

  const mergedPath = path.join(outputDir, "merged-summary.json");
  await fs.writeFile(mergedPath, JSON.stringify(merged, null, 2));
  log.success(`Merged summary written to ${mergedPath}`);

  // Generate HTML for merged report
  const htmlContent = generateHtmlReport(merged, []);
  const htmlPath = path.join(outputDir, "report.html");
  await fs.writeFile(htmlPath, htmlContent);
  log.success(`HTML report written to ${htmlPath}`);
}

async function main() {
  const { values } = parseArgs({
    options: {
      agent: { type: "string" },
      language: { type: "string" },
      scenarios: { type: "string", default: "./scenarios" },
      output: { type: "string", default: "./results" },
      scenario: { type: "string" },
      timeout: { type: "string" },
      model: { type: "string" },
      help: { type: "boolean", short: "h", default: false },
      "dry-run": { type: "boolean", default: false },
      "resume-from": { type: "string" },
      workspace: { type: "string" },
      "merge-reports": { type: "string" },
      "idle-timeout": { type: "string" },
      retries: { type: "string" },
      ctrf: { type: "string" },
    },
  });

  const {
    scenarios,
    output,
    scenario: scenarioFilter,
    timeout,
    help,
    "dry-run": dryRun,
    "resume-from": resumeFrom,
    workspace: workspaceOverride,
    "merge-reports": mergeReportsDir,
  } = values;
  const idleTimeoutArg = values["idle-timeout"];
  const retriesArg = values.retries;
  const ctrfOutputPath = values.ctrf;
  const agentArg = values.agent ?? "all";
  const languageArg = values.language ?? "all";
  const modelArg = values.model;

  // If --model is provided, set OPENCODE_MODEL env var so the driver picks it up
  if (modelArg) {
    process.env.OPENCODE_MODEL = modelArg;
  }

  // Merge-reports mode — standalone, doesn't require --agent/--language/--scenarios
  if (mergeReportsDir) {
    const outputDir = path.resolve(process.cwd(), output!);
    await mergeReports(path.resolve(process.cwd(), mergeReportsDir), outputDir);
    return;
  }

  // Auto-detect GOLEM_PATH if not explicitly set
  if (!process.env.GOLEM_PATH) {
    const detected = await detectGolemWorkspaceRoot();
    if (detected) {
      process.env.GOLEM_PATH = detected;
      log.info(`Auto-detected GOLEM_PATH: ${detected}`);
    }
  }

  if (!process.env.GOLEM_PATH) {
    log.error(
      "GOLEM_PATH is not set and could not be auto-detected.\nSet GOLEM_PATH to the root of your golem repository checkout, or run the harness from within the golem repo tree.",
    );
    process.exit(1);
  }
  const golemPath = process.env.GOLEM_PATH!;

  // Resolve the target directory containing the golem binary and prepend it
  // to PATH so all spawned processes (including agent drivers) use the correct binary.
  const golemTargetDir = resolveGolemTargetDir(golemPath);
  const pathSep = process.platform === "win32" ? ";" : ":";
  process.env.PATH = golemTargetDir + pathSep + (process.env.PATH ?? "");
  log.info(`Using golem binary from: ${golemTargetDir}`);

  if (help) {
    const usage = `
golem-skill-harness — Skill testing harness for Golem coding agents

Usage:
  npx tsx src/run.ts [options]

Options:
  --agent <name>        Agent driver to use (${SUPPORTED_AGENTS.join(", ")}) (default: all)
  --language <lang>     Language for skill templates (${SUPPORTED_LANGUAGES.join(", ")}) (default: all)
  --scenario <name>     Run only the named scenario
  --model <model>       Model identifier to pass to the agent driver (sets OPENCODE_MODEL)
  --scenarios <dir>     Path to scenario YAML files (default: ./scenarios)
  --output <dir>        Results output directory (default: ./results)
  --timeout <seconds>   Global timeout per scenario step in seconds (default: ${DEFAULT_STEP_TIMEOUT_SECONDS})
  --idle-timeout <seconds>  Idle timeout — fail step if agent produces no output for this long (default: ${DEFAULT_IDLE_TIMEOUT_SECONDS})
  --retries <n>             Max scenario retries on idle timeout (default: ${DEFAULT_SCENARIO_RETRIES})
  --dry-run             Validate scenarios and print step summaries without executing
  --resume-from <id>    Resume execution from the given step ID
  --workspace <path>    Override workspace directory
  --merge-reports <dir> Merge summary.json files from <dir> into aggregated report
  --ctrf <path>         Write a CTRF JSON report to the given file path
  -h, --help            Show this help message
`.trim();

    log.usage(usage);
    process.exit(0);
  }

  const agents: SupportedAgent[] =
    agentArg === "all" ? [...SUPPORTED_AGENTS] : [agentArg as SupportedAgent];
  const languages: SupportedLanguage[] =
    languageArg === "all" ? [...SUPPORTED_LANGUAGES] : [languageArg as SupportedLanguage];

  for (const a of agents) {
    if (!SUPPORTED_AGENTS.includes(a)) {
      log.error(`Unsupported agent: ${a}. Supported: ${SUPPORTED_AGENTS.join(", ")}`);
      process.exit(1);
    }
  }
  for (const l of languages) {
    if (!SUPPORTED_LANGUAGES.includes(l)) {
      log.error(`Unsupported language: ${l}. Supported: ${SUPPORTED_LANGUAGES.join(", ")}`);
      process.exit(1);
    }
  }

  const scenariosDir = path.resolve(process.cwd(), scenarios!);
  const resultsDir = path.resolve(process.cwd(), output!);
  const globalTimeoutSeconds = timeout ? Number.parseInt(timeout, 10) : undefined;
  if (resumeFrom && !scenarioFilter) {
    log.error("--resume-from requires --scenario to avoid aborting on unrelated scenarios");
    process.exit(1);
  }

  if (
    globalTimeoutSeconds !== undefined &&
    (!Number.isFinite(globalTimeoutSeconds) || globalTimeoutSeconds <= 0)
  ) {
    log.error(`Invalid --timeout value: ${timeout}`);
    process.exit(1);
  }

  const idleTimeoutSeconds = idleTimeoutArg ? Number.parseInt(idleTimeoutArg, 10) : undefined;
  if (
    idleTimeoutSeconds !== undefined &&
    (!Number.isFinite(idleTimeoutSeconds) || idleTimeoutSeconds <= 0)
  ) {
    log.error(`Invalid --idle-timeout value: ${idleTimeoutArg}`);
    process.exit(1);
  }

  const maxScenarioRetries = retriesArg
    ? Number.parseInt(retriesArg, 10)
    : DEFAULT_SCENARIO_RETRIES;
  if (!Number.isFinite(maxScenarioRetries) || maxScenarioRetries < 0) {
    log.error(`Invalid --retries value: ${retriesArg}`);
    process.exit(1);
  }

  await fs.mkdir(resultsDir, { recursive: true });

  const bootstrapSkillSourceDir = path.join(
    golemPath,
    "golem-skills",
    "skills",
    "common",
    "golem-new-project",
  );
  try {
    await fs.access(path.join(bootstrapSkillSourceDir, "SKILL.md"));
  } catch {
    log.error(`Bootstrap skill not found at ${bootstrapSkillSourceDir}`);
    process.exit(1);
  }

  const scenarioFiles = (await fs.readdir(scenariosDir)).filter(
    (f) => f.endsWith(".yaml") || f.endsWith(".yml"),
  );

  // Validate that the --scenario filter matches an existing scenario
  if (scenarioFilter) {
    const allSpecs = await Promise.all(
      scenarioFiles.map((f) => ScenarioLoader.load(path.join(scenariosDir, f))),
    );
    const scenarioNames = allSpecs.map((s) => s.name);
    if (!scenarioNames.includes(scenarioFilter)) {
      log.error(
        `Scenario '${scenarioFilter}' not found. Available scenarios: ${scenarioNames.join(", ")}`,
      );
      process.exit(1);
    }
  }

  // Dry-run mode: validate and print step summaries, then exit
  if (dryRun) {
    log.bold("=== Dry Run ===");
    for (const file of scenarioFiles) {
      const spec = await ScenarioLoader.load(path.join(scenariosDir, file));
      if (scenarioFilter && spec.name !== scenarioFilter) continue;

      log.heading(`\nScenario: ${spec.name}`);
      log.plain(`  Steps: ${spec.steps.length}`);
      for (let i = 0; i < spec.steps.length; i++) {
        const step = spec.steps[i];
        const label = step.id ?? `step-${i + 1}`;
        const rawPrompt = step.tag === "prompt" ? step.prompt : undefined;
        const promptText =
          typeof rawPrompt === "string"
            ? rawPrompt
            : rawPrompt
              ? JSON.stringify(rawPrompt)
              : undefined;
        let promptPreview: string;
        if (promptText) {
          promptPreview = promptText.length > 60 ? promptText.slice(0, 57) + "..." : promptText;
        } else if (step.tag === "create_project") {
          const cp = step.create_project as Record<string, unknown>;
          promptPreview = `[create_project] ${JSON.stringify(cp)}`;
        } else {
          promptPreview = "(no prompt)";
        }
        const rawSkills = step.expectedSkills;
        const skills =
          (Array.isArray(rawSkills)
            ? rawSkills.join(", ")
            : rawSkills
              ? JSON.stringify(rawSkills)
              : "") || "(none)";
        const timeoutVal = step.timeout ?? spec.settings?.timeout_per_subprompt ?? "default";
        const conditions: string[] = [];
        if (step.only_if) {
          conditions.push(`only_if: ${JSON.stringify(step.only_if)}`);
        }
        if (step.skip_if) {
          conditions.push(`skip_if: ${JSON.stringify(step.skip_if)}`);
        }
        log.dryRunStepLine(label, promptPreview);
        log.dryRunStepDetail(
          `skills: ${skills} | timeout: ${typeof timeoutVal === "number" ? `${timeoutVal}s` : timeoutVal}`,
        );
        if (conditions.length > 0) {
          log.dryRunStepDetail(`conditions: ${conditions.join(", ")}`);
        }
      }
    }
    log.success("\nAll scenarios validated successfully.");
    return;
  }

  // Start Golem server
  const golemServer = new GolemServer();
  const runId = randomUUID();
  const workspacesRoot = workspaceOverride
    ? path.join(path.resolve(process.cwd(), workspaceOverride), runId)
    : path.join(process.cwd(), "workspaces", runId);
  log.dim(`Run ID: ${runId}`);
  const serverDataDir = path.join(workspacesRoot, "golem-server-data");
  log.info("Starting Golem server...");
  await golemServer.start(9881, serverDataDir);
  log.success("Golem server is ready.");

  // Set up graceful Ctrl+C handling
  const abortController = new AbortController();
  let interrupted = false;

  const stopServerAndExit = (code: number) => {
    golemServer.stop().finally(() => {
      process.exit(code);
    });
  };

  process.on("SIGINT", () => {
    if (interrupted) {
      log.error("\nForce exit. Stopping Golem server...");
      stopServerAndExit(130);
      return;
    }
    interrupted = true;
    log.warn(
      "\nInterrupted. Finishing current step and writing partial results... (press Ctrl+C again to force exit)",
    );
    abortController.abort();
  });

  // Ensure server is stopped when the process exits for any reason
  process.on("exit", () => {
    // Synchronous best-effort: send SIGTERM to the server process group
    if (golemServer["serverProcess"]?.pid) {
      try {
        process.kill(-golemServer["serverProcess"].pid, "SIGTERM");
      } catch {
        // Already dead
      }
    }
  });

  const scenarioReports: ScenarioReport[] = [];
  let hasFailures = false;
  let isFirstScenario = true;

  try {
    for (const currentAgent of agents) {
      for (const currentLanguage of languages) {
        const driver = createDriver(currentAgent);
        log.dim(
          `Config: agent=${currentAgent}, language=${currentLanguage}, scenarios=${scenariosDir}, output=${resultsDir}, timeout=${globalTimeoutSeconds ?? "default"}, idle-timeout=${idleTimeoutSeconds ?? "default"}, retries=${maxScenarioRetries}`,
        );

        for (const file of scenarioFiles) {
          // Check if interrupted before starting next scenario
          if (interrupted) {
            log.warn("Skipping remaining scenarios due to interruption.");
            break;
          }

          const spec = await ScenarioLoader.load(path.join(scenariosDir, file));

          if (scenarioFilter && spec.name !== scenarioFilter) continue;

          // Restart Golem server between scenarios to get a clean state
          if (!isFirstScenario) {
            log.dim("Restarting Golem server for clean state...");
            await golemServer.restart();
            log.success("Golem server restarted.");
          }
          isFirstScenario = false;

          log.heading(`Running scenario: ${spec.name} [${currentAgent} x ${currentLanguage}]`);
          const scenarioDir = spec.name.replace(/\s+/g, "-").toLowerCase();

          let scenarioResult: ScenarioRunResult | undefined;
          const totalAttempts = maxScenarioRetries + 1; // retries + initial attempt
          const canRetry = maxScenarioRetries > 0 && !resumeFrom;

          for (let attempt = 1; attempt <= totalAttempts; attempt++) {
            if (attempt > 1) {
              log.scenarioRetry(spec.name, attempt - 1, maxScenarioRetries, "idle timeout");
              // Restart Golem server for clean state
              log.dim("Restarting Golem server for retry...");
              await golemServer.restart();
              log.success("Golem server restarted.");
            }

            const attemptSuffix = attempt > 1 ? `/attempt-${attempt}` : "";
            const workspace = path.join(
              workspacesRoot,
              scenarioDir,
              currentLanguage + attemptSuffix,
            );
            const watcher = new SkillWatcher(workspace);
            const executor = new ScenarioExecutor(
              driver,
              watcher,
              workspace,
              bootstrapSkillSourceDir,
              {
                globalTimeoutSeconds,
                idleTimeoutSeconds,
                agent: currentAgent,
                language: currentLanguage,
                abortSignal: abortController.signal,
                resumeFromStepId: resumeFrom,
              },
            );

            scenarioResult = await executor.execute(spec);

            // Don't retry on success, abort, or non-idle-timeout failures
            if (scenarioResult.status === "pass") break;
            if (interrupted) break;

            if (!canRetry || attempt >= totalAttempts) break;

            const failedDueToIdleTimeout = scenarioResult.stepResults.some(
              (r) => !r.success && r.timedOut && r.timeoutKind === "idle",
            );
            if (!failedDueToIdleTimeout) break;

            log.warn(
              `Scenario "${spec.name}" failed due to idle timeout, will retry (retry ${attempt}/${maxScenarioRetries})`,
            );
          }

          const results = scenarioResult!.stepResults;

          const allPassed = scenarioResult!.status === "pass";
          if (allPassed) {
            log.scenarioPass(spec.name);
          } else {
            hasFailures = true;
            log.scenarioFail(spec.name);
            for (const res of results) {
              if (!res.success) {
                const rawStepName =
                  ("prompt" in res.step && res.step.prompt) || res.step.id || "unnamed";
                const stepName =
                  typeof rawStepName === "string" ? rawStepName : JSON.stringify(rawStepName);
                log.scenarioFailedStep(stepName, res.error ?? "");
                if (res.classification) {
                  log.scenarioFailureClassification(
                    res.classification.category,
                    res.classification.guidance,
                  );
                }
              }
            }
          }

          // Write individual report with agent-language prefix to avoid collisions
          const runId = `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
          const report: ScenarioReport = {
            scenario: spec.name,
            matrix: { agent: currentAgent, language: currentLanguage },
            run_id: runId,
            status: scenarioResult!.status,
            durationSeconds: scenarioResult!.durationSeconds,
            results,
            artifactPaths: scenarioResult!.artifactPaths,
          };

          const reportPath = path.join(
            resultsDir,
            `${currentAgent}-${currentLanguage}-${spec.name}.json`,
          );
          await fs.writeFile(reportPath, JSON.stringify(report, null, 2));
          scenarioReports.push(report);

          log.scenarioResultLine(allPassed, spec.name, results.length, spec.steps.length);
        }
      }
    }

    // Aggregated summary report
    if (scenarioReports.length > 0) {
      const totalScenarios = scenarioReports.length;
      const passed = scenarioReports.filter((r) => r.status === "pass").length;
      const failed = scenarioReports.filter((r) => r.status === "fail").length;
      const totalDuration = scenarioReports.reduce((sum, r) => sum + r.durationSeconds, 0);

      const worstFailures = scenarioReports
        .filter((r) => r.status === "fail")
        .map((r) => {
          const failedStep = r.results.find((s) => !s.success);
          return {
            scenario: r.scenario,
            agent: r.matrix.agent,
            language: r.matrix.language,
            error: failedStep?.error ?? "unknown",
            guidance: failedStep?.classification?.guidance,
          };
        });

      const summary: Summary = {
        agent: agents.join(","),
        language: languages.join(","),
        os: os.platform(),
        timestamp: new Date().toISOString(),
        total: totalScenarios,
        passed,
        failed,
        skipped: 0,
        durationSeconds: totalDuration,
        worstFailures: worstFailures.map((f) => ({
          scenario: f.scenario,
          agent: f.agent,
          language: f.language,
          error: f.error,
        })),
        scenarios: scenarioReports.map((r) => ({
          name: r.scenario,
          status: r.status,
          durationSeconds: r.durationSeconds,
        })),
      };

      const summaryPath = path.join(resultsDir, "summary.json");
      await fs.writeFile(summaryPath, JSON.stringify(summary, null, 2));

      // Generate HTML report
      const htmlContent = generateHtmlReport(summary, scenarioReports as HtmlScenarioReport[]);
      const htmlPath = path.join(resultsDir, "report.html");
      await fs.writeFile(htmlPath, htmlContent);

      // GitHub Actions job summary
      const ghSummaryPath = process.env["GITHUB_STEP_SUMMARY"];
      if (ghSummaryPath) {
        await fs.appendFile(
          ghSummaryPath,
          renderGitHubStepSummary({
            scenarioReports,
            totalScenarios,
            passed,
            failed,
            totalDuration,
            worstFailures: worstFailures.map((failure) => ({
              scenario: failure.scenario,
              matrix: { agent: failure.agent, language: failure.language },
              error: failure.error,
              guidance: failure.guidance,
            })),
          }),
        );
      }

      // Print summary
      log.blank();
      log.bold("=== Test Summary ===");
      log.plain(`Total:    ${totalScenarios}`);
      log.summaryLine("Passed:   ", passed, "green");
      log.summaryLine("Failed:   ", failed, failed > 0 ? "red" : undefined);
      log.plain(`Duration: ${totalDuration.toFixed(1)}s`);

      if (worstFailures.length > 0) {
        log.blank();
        log.error("Failures:");
        for (const f of worstFailures) {
          log.summaryFailure(
            formatScenarioMatrixLabel({
              scenario: f.scenario,
              matrix: { agent: f.agent, language: f.language },
            }),
            f.error,
          );
          if (f.guidance) {
            log.summaryGuidance(f.guidance);
          }
        }
      }

      log.dim(`Reports: ${summaryPath}, ${path.join(resultsDir, "report.html")}`);

      // Generate CTRF report if requested
      if (ctrfOutputPath) {
        const ctrfBuilder = new ReportBuilder({ autoGenerateId: true, autoTimestamp: true })
          .tool({ name: "golem-skill-harness", version: "0.1.0" })
          .environment({
            buildName: `${agents.join(",")}/${languages.join(",")}`,
            ...(process.env.GITHUB_SHA ? { testEnvironment: "ci" } : {}),
          });

        for (const r of scenarioReports) {
          const failedStep = r.results.find((s) => !s.success);
          ctrfBuilder.addTest(
            new TestBuilder()
              .name(r.scenario)
              .status(r.status === "pass" ? "passed" : "failed")
              .duration(Math.round(r.durationSeconds * 1000))
              .suite([r.matrix.agent, r.matrix.language])
              .message(failedStep?.error ?? "")
              .build(),
          );
        }

        const ctrfReport = ctrfBuilder.build();
        const resolvedCtrfPath = path.resolve(process.cwd(), ctrfOutputPath);
        await fs.mkdir(path.dirname(resolvedCtrfPath), { recursive: true });
        await fs.writeFile(resolvedCtrfPath, stringify(ctrfReport));
        log.dim(`CTRF report: ${resolvedCtrfPath}`);
      }
    }

    if (hasFailures) {
      process.exitCode = 1;
    }
  } finally {
    await golemServer.stop();
  }
}

main().catch(async (err) => {
  log.fatal("Fatal error:");
  log.error(err instanceof Error ? (err.stack ?? `${err.name}: ${err.message}`) : String(err));
  // Server cleanup is handled by the finally block in main(), but if main() itself
  // throws before reaching it, we still need to exit.
  process.exit(1);
});

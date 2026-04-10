import { parseArgs } from "node:util";
import * as path from "node:path";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import { randomUUID } from "node:crypto";
import {
  ScenarioLoader,
  ScenarioExecutor,
  DEFAULT_STEP_TIMEOUT_SECONDS,
  type ScenarioRunResult,
} from "./executor.js";
import { AmpAgentDriver } from "./driver/amp.js";
import { ClaudeAgentDriver } from "./driver/claude.js";
import { OpenCodeAgentDriver } from "./driver/opencode.js";
import { CodexAgentDriver } from "./driver/codex.js";
import type { AgentDriver } from "./driver/base.js";
import { SkillWatcher } from "./watcher.js";
import {
  generateHtmlReport,
  type Summary,
  type MergedSummary,
  type HtmlScenarioReport,
} from "./html-report.js";
import * as log from "./log.js";
import { detectGolemWorkspaceRoot, resolveGolemTargetDir, GolemServer } from "./workspace.js";

const SUPPORTED_AGENTS = [
  "amp",
  "claude-code",
  "opencode",
  "codex",
] as const;
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
  }
}


async function mergeReports(
  reportsDir: string,
  outputDir: string,
): Promise<void> {
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
      skills: { type: "string", default: "../../skills" },
      help: { type: "boolean", short: "h", default: false },
      "dry-run": { type: "boolean", default: false },
      "resume-from": { type: "string" },
      workspace: { type: "string" },
      "merge-reports": { type: "string" },
    },
  });

  const {
    scenarios,
    output,
    scenario: scenarioFilter,
    timeout,
    skills: skillsDirRel,
    help,
    "dry-run": dryRun,
    "resume-from": resumeFrom,
    workspace: workspaceOverride,
    "merge-reports": mergeReportsDir,
  } = values;
  const agentArg = values.agent ?? "all";
  const languageArg = values.language ?? "all";

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
    log.error("GOLEM_PATH is not set and could not be auto-detected.\nSet GOLEM_PATH to the root of your golem repository checkout, or run the harness from within the golem repo tree.");
    process.exit(1);
  }

  // Resolve the target directory containing the golem binary and prepend it
  // to PATH so all spawned processes (including agent drivers) use the correct binary.
  const golemTargetDir = resolveGolemTargetDir(process.env.GOLEM_PATH);
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
  --scenarios <dir>     Path to scenario YAML files (default: ./scenarios)
  --output <dir>        Results output directory (default: ./results)
  --timeout <seconds>   Global timeout per scenario step in seconds (default: ${DEFAULT_STEP_TIMEOUT_SECONDS})
  --skills <dir>        Path to skills directory (default: ../../skills)
  --dry-run             Validate scenarios and print step summaries without executing
  --resume-from <id>    Resume execution from the given step ID
  --workspace <path>    Override workspace directory
  --merge-reports <dir> Merge summary.json files from <dir> into aggregated report
  -h, --help            Show this help message
`.trim();

    log.usage(usage);
    process.exit(0);
  }

  const agents: SupportedAgent[] =
    agentArg === "all" ? [...SUPPORTED_AGENTS] : [agentArg as SupportedAgent];
  const languages: SupportedLanguage[] =
    languageArg === "all"
      ? [...SUPPORTED_LANGUAGES]
      : [languageArg as SupportedLanguage];

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

  const skillsDir = path.resolve(process.cwd(), skillsDirRel!);
  const scenariosDir = path.resolve(process.cwd(), scenarios!);
  const resultsDir = path.resolve(process.cwd(), output!);
  const globalTimeoutSeconds = timeout
    ? Number.parseInt(timeout, 10)
    : undefined;
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

  await fs.mkdir(resultsDir, { recursive: true });

  const scenarioFiles = (await fs.readdir(scenariosDir)).filter(
    (f) => f.endsWith(".yaml") || f.endsWith(".yml"),
  );

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
        const promptText = typeof rawPrompt === "string" ? rawPrompt : rawPrompt ? JSON.stringify(rawPrompt) : undefined;
        const promptPreview = promptText
          ? promptText.length > 60
            ? promptText.slice(0, 57) + "..."
            : promptText
          : "(no prompt)";
        const rawSkills = step.expectedSkills;
        const skills = (Array.isArray(rawSkills) ? rawSkills.join(", ") : rawSkills ? JSON.stringify(rawSkills) : "") || "(none)";
        const timeoutVal =
          step.timeout ?? spec.settings?.timeout_per_subprompt ?? "default";
        const conditions: string[] = [];
        if (step.only_if) {
          conditions.push(`only_if: ${JSON.stringify(step.only_if)}`);
        }
        if (step.skip_if) {
          conditions.push(`skip_if: ${JSON.stringify(step.skip_if)}`);
        }
        log.dryRunStepLine(label, promptPreview);
        log.dryRunStepDetail(`skills: ${skills} | timeout: ${typeof timeoutVal === "number" ? `${timeoutVal}s` : timeoutVal}`);
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
  const workspacesRoot = path.join(process.cwd(), "workspaces", runId);
  log.dim(`Run ID: ${runId}`);
  const serverDataDir = path.join(workspacesRoot, "golem-server-data");
  log.info("Starting Golem server...");
  await golemServer.start(9881, serverDataDir);
  log.success("Golem server is ready.");

  // Set up graceful Ctrl+C handling
  const abortController = new AbortController();
  let interrupted = false;

  process.on("SIGINT", () => {
    if (interrupted) {
      golemServer.stop().finally(() => {
        log.error("\nForce exit.");
        process.exit(130);
      });
      return;
    }
    interrupted = true;
    log.warn("\nInterrupted. Finishing current step and writing partial results... (press Ctrl+C again to force exit)");
    abortController.abort();
  });

  const scenarioReports: ScenarioReport[] = [];
  let hasFailures = false;
  let isFirstScenario = true;

  try {
  for (const currentAgent of agents) {
    for (const currentLanguage of languages) {
      const driver = createDriver(currentAgent);
      const watcher = new SkillWatcher(skillsDir);
      log.dim(`Config: agent=${currentAgent}, language=${currentLanguage}, scenarios=${scenariosDir}, output=${resultsDir}, timeout=${globalTimeoutSeconds ?? "default"}`);

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
        const workspace = workspaceOverride
          ? path.resolve(process.cwd(), workspaceOverride)
          : path.join(
              workspacesRoot,
              spec.name.replace(/\s+/g, "-").toLowerCase(),
              currentLanguage,
            );
        const executor = new ScenarioExecutor(
          driver,
          watcher,
          workspace,
          skillsDir,
          {
            globalTimeoutSeconds,
            agent: currentAgent,
            language: currentLanguage,
            abortSignal: abortController.signal,
            resumeFromStepId: resumeFrom,
          },
        );

        const scenarioResult = await executor.execute(spec);
        const results = scenarioResult.stepResults;

        const allPassed = scenarioResult.status === "pass";
        if (allPassed) {
          log.scenarioPass(spec.name);
        } else {
          hasFailures = true;
          log.scenarioFail(spec.name);
          for (const res of results) {
            if (!res.success) {
              log.scenarioFailedStep(String(("prompt" in res.step && res.step.prompt) || res.step.id || "unnamed"), res.error ?? "");
              if (res.classification) {
                log.scenarioFailureClassification(res.classification.category, res.classification.guidance);
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
          status: scenarioResult.status,
          durationSeconds: scenarioResult.durationSeconds,
          results,
          artifactPaths: scenarioResult.artifactPaths,
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
    const totalDuration = scenarioReports.reduce(
      (sum, r) => sum + r.durationSeconds,
      0,
    );

    const worstFailures = scenarioReports
      .filter((r) => r.status === "fail")
      .map((r) => {
        const failedStep = r.results.find((s) => !s.success);
        return {
          scenario: r.scenario,
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
    const htmlContent = generateHtmlReport(
      summary,
      scenarioReports as HtmlScenarioReport[],
    );
    const htmlPath = path.join(resultsDir, "report.html");
    await fs.writeFile(htmlPath, htmlContent);

    // GitHub Actions job summary
    const ghSummaryPath = process.env["GITHUB_STEP_SUMMARY"];
    if (ghSummaryPath) {
      const lines: string[] = [];
      lines.push("## Skill Test Results");
      lines.push("");
      lines.push("| Scenario | Agent | Language | Status | Duration |");
      lines.push("|----------|-------|----------|--------|----------|");
      for (const r of scenarioReports) {
        const icon = r.status === "pass" ? "\u2705" : "\u274c";
        lines.push(
          `| ${r.scenario} | ${r.matrix.agent} | ${r.matrix.language} | ${icon} ${r.status} | ${r.durationSeconds.toFixed(1)}s |`,
        );
      }
      lines.push("");
      lines.push(
        `**Total:** ${totalScenarios} | **Passed:** ${passed} | **Failed:** ${failed} | **Duration:** ${totalDuration.toFixed(1)}s`,
      );

      if (worstFailures.length > 0) {
        lines.push("");
        lines.push("### Failures");
        for (const f of worstFailures) {
          const truncatedError =
            f.error.length > 200 ? f.error.slice(0, 197) + "..." : f.error;
          lines.push(`- **${f.scenario}**: ${truncatedError}`);
          if (f.guidance) {
            lines.push(`  - _${f.guidance}_`);
          }
        }
      }
      lines.push("");

      await fs.appendFile(ghSummaryPath, lines.join("\n"));
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
        log.summaryFailure(f.scenario, f.error);
        if (f.guidance) {
          log.summaryGuidance(f.guidance);
        }
      }
    }

    log.dim(`Reports: ${summaryPath}, ${path.join(resultsDir, "report.html")}`);
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
  process.exit(1);
});

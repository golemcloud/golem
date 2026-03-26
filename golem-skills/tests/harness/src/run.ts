import { parseArgs } from "node:util";
import * as path from "node:path";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import { spawn } from "node:child_process";
import {
  ScenarioLoader,
  ScenarioExecutor,
  DEFAULT_STEP_TIMEOUT_SECONDS,
  type ScenarioRunResult,
} from "./executor.js";
import { ClaudeAgentDriver } from "./driver/claude.js";
import { GeminiAgentDriver } from "./driver/gemini.js";
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
import chalk from "chalk";

const SUPPORTED_AGENTS = [
  "claude-code",
  "gemini",
  "opencode",
  "codex",
] as const;
const SUPPORTED_LANGUAGES = ["ts", "rust"] as const;

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
    case "claude-code":
      return new ClaudeAgentDriver();
    case "gemini":
      return new GeminiAgentDriver();
    case "opencode":
      return new OpenCodeAgentDriver();
    case "codex":
      return new CodexAgentDriver();
  }
}

async function cleanupGolemState(cwd: string): Promise<void> {
  // Find a subdirectory containing golem.yaml (the app dir is one level inside the workspace)
  const fsSync = await import("node:fs");
  let appDir = cwd;
  if (!fsSync.existsSync(path.join(cwd, "golem.yaml"))) {
    const entries = fsSync.readdirSync(cwd, { withFileTypes: true });
    for (const entry of entries) {
      if (
        entry.isDirectory() &&
        fsSync.existsSync(path.join(cwd, entry.name, "golem.yaml"))
      ) {
        appDir = path.join(cwd, entry.name);
        break;
      }
    }
  }
  return new Promise((resolve) => {
    const child = spawn("golem", ["deploy", "--reset", "--yes"], {
      cwd: appDir,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let output = "";
    child.stdout?.on("data", (data: Buffer) => (output += data.toString()));
    child.stderr?.on("data", (data: Buffer) => (output += data.toString()));
    child.on("close", (code) => {
      if (code !== 0) {
        console.warn(
          chalk.yellow(
            `Warning: golem deploy --reset failed (exit ${code}): ${output.trim()}`,
          ),
        );
      }
      resolve();
    });
    child.on("error", (err) => {
      console.warn(
        chalk.yellow(`Warning: golem deploy --reset error: ${err.message}`),
      );
      resolve();
    });
  });
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
    console.error(
      chalk.red("No summary.json files found in the reports directory"),
    );
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
  console.log(chalk.green(`Merged summary written to ${mergedPath}`));

  // Generate HTML for merged report
  const htmlContent = generateHtmlReport(merged, []);
  const htmlPath = path.join(outputDir, "report.html");
  await fs.writeFile(htmlPath, htmlContent);
  console.log(chalk.green(`HTML report written to ${htmlPath}`));
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
      "no-cleanup": { type: "boolean", default: false },
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
    "no-cleanup": noCleanup,
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
  --workspace <path>    Override workspace directory (implies --no-cleanup)
  --no-cleanup          Skip Golem state cleanup between scenarios
  --merge-reports <dir> Merge summary.json files from <dir> into aggregated report
  -h, --help            Show this help message
`.trim();

    console.log(usage);
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
      console.error(
        chalk.red(
          `Unsupported agent: ${a}. Supported: ${SUPPORTED_AGENTS.join(", ")}`,
        ),
      );
      process.exit(1);
    }
  }
  for (const l of languages) {
    if (!SUPPORTED_LANGUAGES.includes(l)) {
      console.error(
        chalk.red(
          `Unsupported language: ${l}. Supported: ${SUPPORTED_LANGUAGES.join(", ")}`,
        ),
      );
      process.exit(1);
    }
  }

  const skillsDir = path.resolve(process.cwd(), skillsDirRel!);
  const scenariosDir = path.resolve(process.cwd(), scenarios!);
  const resultsDir = path.resolve(process.cwd(), output!);
  const globalTimeoutSeconds = timeout
    ? Number.parseInt(timeout, 10)
    : undefined;
  const skipCleanup = noCleanup || !!workspaceOverride;

  if (resumeFrom && !scenarioFilter) {
    console.error(
      chalk.red("--resume-from requires --scenario to avoid aborting on unrelated scenarios"),
    );
    process.exit(1);
  }

  if (
    globalTimeoutSeconds !== undefined &&
    (!Number.isFinite(globalTimeoutSeconds) || globalTimeoutSeconds <= 0)
  ) {
    console.error(chalk.red(`Invalid --timeout value: ${timeout}`));
    process.exit(1);
  }

  await fs.mkdir(resultsDir, { recursive: true });

  const scenarioFiles = (await fs.readdir(scenariosDir)).filter(
    (f) => f.endsWith(".yaml") || f.endsWith(".yml"),
  );

  // Dry-run mode: validate and print step summaries, then exit
  if (dryRun) {
    console.log(chalk.bold("=== Dry Run ==="));
    for (const file of scenarioFiles) {
      const spec = await ScenarioLoader.load(path.join(scenariosDir, file));
      if (scenarioFilter && spec.name !== scenarioFilter) continue;

      console.log(chalk.blue(`\nScenario: ${spec.name}`));
      console.log(`  Steps: ${spec.steps.length}`);
      for (let i = 0; i < spec.steps.length; i++) {
        const step = spec.steps[i];
        const label = step.id ?? `step-${i + 1}`;
        const promptPreview = step.prompt
          ? step.prompt.length > 60
            ? step.prompt.slice(0, 57) + "..."
            : step.prompt
          : "(no prompt)";
        const skills = step.expectedSkills?.join(", ") || "(none)";
        const timeoutVal =
          step.timeout ?? spec.settings?.timeout_per_subprompt ?? "default";
        const conditions: string[] = [];
        if (step.only_if) {
          conditions.push(`only_if: ${JSON.stringify(step.only_if)}`);
        }
        if (step.skip_if) {
          conditions.push(`skip_if: ${JSON.stringify(step.skip_if)}`);
        }
        console.log(`  [${label}] ${promptPreview}`);
        console.log(
          `    skills: ${skills} | timeout: ${typeof timeoutVal === "number" ? `${timeoutVal}s` : timeoutVal}`,
        );
        if (conditions.length > 0) {
          console.log(`    conditions: ${conditions.join(", ")}`);
        }
      }
    }
    console.log(chalk.green("\nAll scenarios validated successfully."));
    return;
  }

  // Set up graceful Ctrl+C handling
  const abortController = new AbortController();
  let interrupted = false;

  process.on("SIGINT", () => {
    if (interrupted) {
      console.log(chalk.red("\nForce exit."));
      process.exit(130);
    }
    interrupted = true;
    console.log(
      chalk.yellow(
        "\nInterrupted. Finishing current step and writing partial results... (press Ctrl+C again to force exit)",
      ),
    );
    abortController.abort();
  });

  const scenarioReports: ScenarioReport[] = [];
  let hasFailures = false;
  let isFirstScenario = true;
  let lastWorkspace: string | undefined;

  for (const currentAgent of agents) {
    for (const currentLanguage of languages) {
      const driver = createDriver(currentAgent);
      const watcher = new SkillWatcher(skillsDir);
      console.log(
        chalk.gray(
          `Config: agent=${currentAgent}, language=${currentLanguage}, scenarios=${scenariosDir}, output=${resultsDir}, timeout=${globalTimeoutSeconds ?? "default"}`,
        ),
      );

      for (const file of scenarioFiles) {
        // Check if interrupted before starting next scenario
        if (interrupted) {
          console.log(
            chalk.yellow(`Skipping remaining scenarios due to interruption.`),
          );
          break;
        }

        const spec = await ScenarioLoader.load(path.join(scenariosDir, file));

        if (scenarioFilter && spec.name !== scenarioFilter) continue;

        // Cleanup Golem state between scenarios (run in previous workspace where golem.yaml exists)
        if (!isFirstScenario && !skipCleanup && lastWorkspace) {
          console.log(
            chalk.gray("Cleaning up Golem state between scenarios..."),
          );
          await cleanupGolemState(lastWorkspace);
        }
        isFirstScenario = false;

        console.log(
          chalk.blue(
            `Running scenario: ${spec.name} [${currentAgent} x ${currentLanguage}]`,
          ),
        );
        const workspace = workspaceOverride
          ? path.resolve(process.cwd(), workspaceOverride)
          : path.join(
              process.cwd(),
              "workspaces",
              spec.name.replace(/\s+/g, "-").toLowerCase(),
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
            skipCleanup,
          },
        );

        const scenarioResult = await executor.execute(spec);
        lastWorkspace = workspace;
        const results = scenarioResult.stepResults;

        const allPassed = scenarioResult.status === "pass";
        if (allPassed) {
          console.log(chalk.green(`Scenario ${spec.name} PASSED`));
        } else {
          hasFailures = true;
          console.log(chalk.red(`Scenario ${spec.name} FAILED`));
          for (const res of results) {
            if (!res.success) {
              console.log(
                chalk.red(
                  `  Step failed: ${("prompt" in res.step && res.step.prompt) || res.step.id || "unnamed"}`,
                ),
              );
              console.log(chalk.red(`  Error: ${res.error}`));
              if (res.classification) {
                console.log(
                  chalk.yellow(
                    `  [${res.classification.category}] ${res.classification.guidance}`,
                  ),
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

        console.log(
          `${allPassed ? "PASS" : "FAIL"} ${spec.name} steps=${results.length}/${spec.steps.length}`,
        );
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
    console.log("");
    console.log(chalk.bold("=== Test Summary ==="));
    console.log(`Total:    ${totalScenarios}`);
    console.log(chalk.green(`Passed:   ${passed}`));
    if (failed > 0) {
      console.log(chalk.red(`Failed:   ${failed}`));
    } else {
      console.log(`Failed:   ${failed}`);
    }
    console.log(`Duration: ${totalDuration.toFixed(1)}s`);

    if (worstFailures.length > 0) {
      console.log("");
      console.log(chalk.red("Failures:"));
      for (const f of worstFailures) {
        console.log(chalk.red(`  ${f.scenario}: ${f.error}`));
        if (f.guidance) {
          console.log(chalk.yellow(`    ${f.guidance}`));
        }
      }
    }

    console.log(
      chalk.gray(
        `Reports: ${summaryPath}, ${path.join(resultsDir, "report.html")}`,
      ),
    );
  }

  if (hasFailures) {
    process.exit(1);
  }
}

main().catch((err) => {
  console.error(chalk.red("Fatal error:"));
  console.error(err);
  process.exit(1);
});

import { parseArgs } from "node:util";
import * as path from "node:path";
import * as fs from "node:fs/promises";
import {
  ScenarioLoader,
  ScenarioExecutor,
  type ScenarioRunResult,
} from "./executor.js";
import { ClaudeAgentDriver } from "./driver/claude.js";
import { GeminiAgentDriver } from "./driver/gemini.js";
import { OpenCodeAgentDriver } from "./driver/opencode.js";
import { SkillWatcher } from "./watcher.js";
import chalk from "chalk";

interface ScenarioReport {
  scenario: string;
  matrix: { agent: string; language: string };
  run_id: string;
  status: "pass" | "fail";
  durationSeconds: number;
  results: ScenarioRunResult["stepResults"];
  artifactPaths: string[];
}

async function main() {
  const { values } = parseArgs({
    options: {
      agent: { type: "string" },
      language: { type: "string" },
      scenarios: { type: "string" },
      output: { type: "string", default: "./results" },
      scenario: { type: "string" },
      timeout: { type: "string" },
      skills: { type: "string", default: "../../skills" },
      "dry-run": { type: "boolean", default: false },
    },
  });

  const {
    agent,
    language,
    scenarios,
    output,
    scenario: scenarioFilter,
    timeout,
    skills: skillsDirRel,
    "dry-run": dryRun,
  } = values;

  // In dry-run mode, only --scenarios is required
  if (dryRun) {
    if (!scenarios) {
      console.error(chalk.red("Usage: run.ts --dry-run --scenarios <dir>"));
      process.exit(1);
    }
  } else if (!agent || !language || !scenarios) {
    console.error(
      chalk.red(
        "Usage: run.ts --agent <name> --language <ts|rust> --scenarios <dir> [--scenario <name>] [--skills <dir>] [--dry-run]",
      ),
    );
    process.exit(1);
  }

  const skillsDir = path.resolve(process.cwd(), skillsDirRel!);
  const scenariosDir = path.resolve(process.cwd(), scenarios!);
  const resultsDir = path.resolve(process.cwd(), output!);
  const globalTimeoutSeconds = timeout
    ? Number.parseInt(timeout, 10)
    : undefined;

  if (
    globalTimeoutSeconds !== undefined &&
    (!Number.isFinite(globalTimeoutSeconds) || globalTimeoutSeconds <= 0)
  ) {
    console.error(chalk.red(`Invalid --timeout value: ${timeout}`));
    process.exit(1);
  }

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

  await fs.mkdir(resultsDir, { recursive: true });

  let driver;
  if (agent === "claude-code") {
    driver = new ClaudeAgentDriver();
  } else if (agent === "gemini") {
    driver = new GeminiAgentDriver();
  } else if (agent === "opencode") {
    driver = new OpenCodeAgentDriver();
  } else {
    console.error(chalk.red(`Unsupported agent: ${agent}`));
    process.exit(1);
  }

  const watcher = new SkillWatcher(skillsDir);
  console.log(
    chalk.gray(
      `Config: agent=${agent}, language=${language}, scenarios=${scenariosDir}, output=${resultsDir}, timeout=${globalTimeoutSeconds ?? "default"}`,
    ),
  );

  const scenarioReports: ScenarioReport[] = [];
  let hasFailures = false;

  try {
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

      console.log(chalk.blue(`Running scenario: ${spec.name}`));
      const workspace = path.join(
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
          agent,
          language,
          abortSignal: abortController.signal,
        },
      );

      const scenarioResult = await executor.execute(spec);
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
                `  Step failed: ${res.step.prompt || res.step.id || "unnamed"}`,
              ),
            );
            console.log(chalk.red(`  Error: ${res.error}`));
          }
        }
      }

      // Write individual report
      const runId = `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
      const report: ScenarioReport = {
        scenario: spec.name,
        matrix: { agent: agent!, language: language! },
        run_id: runId,
        status: scenarioResult.status,
        durationSeconds: scenarioResult.durationSeconds,
        results,
        artifactPaths: scenarioResult.artifactPaths,
      };

      const reportPath = path.join(resultsDir, `${spec.name}.json`);
      await fs.writeFile(reportPath, JSON.stringify(report, null, 2));
      scenarioReports.push(report);

      console.log(
        `${allPassed ? "PASS" : "FAIL"} ${spec.name} steps=${results.length}/${spec.steps.length}`,
      );
    }
  } finally {
    // Always write summary, even on interruption
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
          };
        });

      const summary = {
        total: totalScenarios,
        passed,
        failed,
        skipped: 0,
        durationSeconds: totalDuration,
        worstFailures,
        scenarios: scenarioReports.map((r) => ({
          name: r.scenario,
          status: r.status,
          durationSeconds: r.durationSeconds,
        })),
      };

      const summaryPath = path.join(resultsDir, "summary.json");
      await fs.writeFile(summaryPath, JSON.stringify(summary, null, 2));

      // GitHub Actions job summary
      const ghSummaryPath = process.env["GITHUB_STEP_SUMMARY"];
      if (ghSummaryPath) {
        const lines: string[] = [];
        lines.push(`## Skill Test Results — ${agent} / ${language}`);
        lines.push("");
        lines.push("| Scenario | Status | Duration |");
        lines.push("|----------|--------|----------|");
        for (const r of scenarioReports) {
          const icon = r.status === "pass" ? "\u2705" : "\u274c";
          lines.push(
            `| ${r.scenario} | ${icon} ${r.status} | ${r.durationSeconds.toFixed(1)}s |`,
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
        }
      }
    }
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

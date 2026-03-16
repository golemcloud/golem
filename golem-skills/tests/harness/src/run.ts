import { parseArgs } from "node:util";
import * as path from "node:path";
import * as fs from "node:fs/promises";
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
    },
  });

  const {
    scenarios,
    output,
    scenario: scenarioFilter,
    timeout,
    skills: skillsDirRel,
    help,
  } = values;
  const agentArg = values.agent ?? "all";
  const languageArg = values.language ?? "all";

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

  const scenarioReports: ScenarioReport[] = [];
  let hasFailures = false;

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
        const spec = await ScenarioLoader.load(path.join(scenariosDir, file));

        if (scenarioFilter && spec.name !== scenarioFilter) continue;

        console.log(
          chalk.blue(
            `Running scenario: ${spec.name} [${currentAgent} x ${currentLanguage}]`,
          ),
        );
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
          { globalTimeoutSeconds },
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
                  `  Step failed: ${("prompt" in res.step && res.step.prompt) || res.step.id || "unnamed"}`,
                ),
              );
              console.log(chalk.red(`  Error: ${res.error}`));
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

  if (hasFailures) {
    process.exit(1);
  }
}

main().catch((err) => {
  console.error(chalk.red("Fatal error:"));
  console.error(err);
  process.exit(1);
});

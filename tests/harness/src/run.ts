import { parseArgs } from 'node:util';
import * as path from 'node:path';
import * as fs from 'node:fs/promises';
import { ScenarioLoader, ScenarioExecutor, type ScenarioRunResult } from './executor.js';
import { ClaudeAgentDriver } from './driver/claude.js';
import { GeminiAgentDriver } from './driver/gemini.js';
import { OpenCodeAgentDriver } from './driver/opencode.js';
import { CodexAgentDriver } from './driver/codex.js';
import { SkillWatcher } from './watcher.js';
import chalk from 'chalk';

interface ScenarioReport {
  scenario: string;
  matrix: { agent: string; language: string };
  run_id: string;
  status: 'pass' | 'fail';
  durationSeconds: number;
  results: ScenarioRunResult['stepResults'];
  artifactPaths: string[];
}

async function main() {
  const { values } = parseArgs({
    options: {
      agent: { type: 'string' },
      language: { type: 'string' },
      scenarios: { type: 'string', default: './scenarios' },
      output: { type: 'string', default: './results' },
      scenario: { type: 'string' },
      timeout: { type: 'string' },
      skills: { type: 'string', default: '../../skills' },
      help: { type: 'boolean', short: 'h', default: false }
    }
  });

  const { agent, language, scenarios, output, scenario: scenarioFilter, timeout, skills: skillsDirRel, help } = values;

  if (help || !agent || !language) {
    const usage = `
golem-skill-harness — Skill testing harness for Golem coding agents

Usage:
  npx tsx src/run.ts --agent <name> --language <ts|rust> --scenarios <dir> [options]

Required:
  --agent <name>        Agent driver to use (claude-code, gemini, opencode, codex)
  --language <lang>     Language for skill templates (ts, rust)

Options:
  --scenario <name>     Run only the named scenario
  --scenarios <dir>     Path to scenario YAML files (default: ./scenarios)
  --output <dir>        Results output directory (default: ./results)
  --timeout <seconds>   Global timeout per scenario in seconds
  --skills <dir>        Path to skills directory (default: ../../skills)
  -h, --help            Show this help message
`.trim();

    if (help) {
      console.log(usage);
      process.exit(0);
    }
    console.error(chalk.red(usage));
    process.exit(1);
  }

  const skillsDir = path.resolve(process.cwd(), skillsDirRel!);
  const scenariosDir = path.resolve(process.cwd(), scenarios!);
  const resultsDir = path.resolve(process.cwd(), output!);
  const globalTimeoutSeconds = timeout ? Number.parseInt(timeout, 10) : undefined;

  if (globalTimeoutSeconds !== undefined && (!Number.isFinite(globalTimeoutSeconds) || globalTimeoutSeconds <= 0)) {
    console.error(chalk.red(`Invalid --timeout value: ${timeout}`));
    process.exit(1);
  }

  await fs.mkdir(resultsDir, { recursive: true });

  let driver;
  if (agent === 'claude-code') {
    driver = new ClaudeAgentDriver();
  } else if (agent === 'gemini') {
    driver = new GeminiAgentDriver();
  } else if (agent === 'opencode') {
    driver = new OpenCodeAgentDriver();
  } else if (agent === 'codex') {
    driver = new CodexAgentDriver();
  } else {
    console.error(chalk.red(`Unsupported agent: ${agent}`));
    process.exit(1);
  }

  const watcher = new SkillWatcher(skillsDir);
  const scenarioFiles = (await fs.readdir(scenariosDir)).filter(f => f.endsWith('.yaml') || f.endsWith('.yml'));
  console.log(chalk.gray(`Config: agent=${agent}, language=${language}, scenarios=${scenariosDir}, output=${resultsDir}, timeout=${globalTimeoutSeconds ?? 'default'}`));

  const scenarioReports: ScenarioReport[] = [];
  let hasFailures = false;

  for (const file of scenarioFiles) {
    const spec = await ScenarioLoader.load(path.join(scenariosDir, file));

    if (scenarioFilter && spec.name !== scenarioFilter) continue;

    console.log(chalk.blue(`Running scenario: ${spec.name}`));
    const workspace = path.join(process.cwd(), 'workspaces', spec.name.replace(/\s+/g, '-').toLowerCase());
    const executor = new ScenarioExecutor(driver, watcher, workspace, skillsDir, { globalTimeoutSeconds });

    const scenarioResult = await executor.execute(spec);
    const results = scenarioResult.stepResults;

    const allPassed = scenarioResult.status === 'pass';
    if (allPassed) {
      console.log(chalk.green(`Scenario ${spec.name} PASSED`));
    } else {
      hasFailures = true;
      console.log(chalk.red(`Scenario ${spec.name} FAILED`));
      for (const res of results) {
        if (!res.success) {
          console.log(chalk.red(`  Step failed: ${res.step.prompt || res.step.id || 'unnamed'}`));
          console.log(chalk.red(`  Error: ${res.error}`));
        }
      }
    }

    // Write individual report
    const runId = `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
    const report: ScenarioReport = {
      scenario: spec.name,
      matrix: { agent, language },
      run_id: runId,
      status: scenarioResult.status,
      durationSeconds: scenarioResult.durationSeconds,
      results,
      artifactPaths: scenarioResult.artifactPaths,
    };

    const reportPath = path.join(resultsDir, `${spec.name}.json`);
    await fs.writeFile(reportPath, JSON.stringify(report, null, 2));
    scenarioReports.push(report);

    console.log(`${allPassed ? 'PASS' : 'FAIL'} ${spec.name} steps=${results.length}/${spec.steps.length}`);
  }

  // Aggregated summary report (#2912)
  if (scenarioReports.length > 0) {
    const totalScenarios = scenarioReports.length;
    const passed = scenarioReports.filter(r => r.status === 'pass').length;
    const failed = scenarioReports.filter(r => r.status === 'fail').length;
    const totalDuration = scenarioReports.reduce((sum, r) => sum + r.durationSeconds, 0);

    const worstFailures = scenarioReports
      .filter(r => r.status === 'fail')
      .map(r => {
        const failedStep = r.results.find(s => !s.success);
        return {
          scenario: r.scenario,
          error: failedStep?.error ?? 'unknown',
        };
      });

    const summary = {
      total: totalScenarios,
      passed,
      failed,
      skipped: 0,
      durationSeconds: totalDuration,
      worstFailures,
      scenarios: scenarioReports.map(r => ({
        name: r.scenario,
        status: r.status,
        durationSeconds: r.durationSeconds,
      })),
    };

    const summaryPath = path.join(resultsDir, 'summary.json');
    await fs.writeFile(summaryPath, JSON.stringify(summary, null, 2));

    // Print summary
    console.log('');
    console.log(chalk.bold('=== Test Summary ==='));
    console.log(`Total:    ${totalScenarios}`);
    console.log(chalk.green(`Passed:   ${passed}`));
    if (failed > 0) {
      console.log(chalk.red(`Failed:   ${failed}`));
    } else {
      console.log(`Failed:   ${failed}`);
    }
    console.log(`Duration: ${totalDuration.toFixed(1)}s`);

    if (worstFailures.length > 0) {
      console.log('');
      console.log(chalk.red('Failures:'));
      for (const f of worstFailures) {
        console.log(chalk.red(`  ${f.scenario}: ${f.error}`));
      }
    }
  }

  if (hasFailures) {
    process.exit(1);
  }
}

main().catch(err => {
  console.error(chalk.red('Fatal error:'));
  console.error(err);
  process.exit(1);
});

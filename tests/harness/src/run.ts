import { parseArgs } from 'node:util';
import * as path from 'node:path';
import * as fs from 'node:fs/promises';
import { ScenarioLoader, ScenarioExecutor } from './executor.js';
import { ClaudeAgentDriver } from './driver/claude.js';
import { GeminiAgentDriver } from './driver/gemini.js';
import { SkillWatcher } from './watcher.js';
import chalk from 'chalk';

async function main() {
  const { values } = parseArgs({
    options: {
      agent: { type: 'string' },
      language: { type: 'string' },
      scenarios: { type: 'string' },
      output: { type: 'string', default: './results' },
      scenario: { type: 'string' },
      timeout: { type: 'string' },
      skills: { type: 'string', default: '../../.agents/skills' }
    }
  });

  const { agent, language, scenarios, output, scenario: scenarioFilter, timeout, skills: skillsDirRel } = values;

  if (!agent || !language || !scenarios) {
    console.error(chalk.red('Usage: run.ts --agent <name> --language <ts|rust> --scenarios <dir> [--scenario <name>] [--skills <dir>]'));
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
  } else {
    console.error(chalk.red(`Unsupported agent: ${agent}`));
    process.exit(1);
  }

  const watcher = new SkillWatcher(skillsDir);
  const scenarioFiles = (await fs.readdir(scenariosDir)).filter(f => f.endsWith('.yaml') || f.endsWith('.yml'));
  console.log(chalk.gray(`Config: agent=${agent}, language=${language}, scenarios=${scenariosDir}, output=${resultsDir}, timeout=${globalTimeoutSeconds ?? 'default'}`));

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
          console.log(chalk.red(`  Step failed: ${res.step.prompt || 'unnamed'}`));
          console.log(chalk.red(`  Error: ${res.error}`));
        }
      }
    }

    // Write report
    const reportPath = path.join(resultsDir, `${spec.name}.json`);
    const runId = `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
    await fs.writeFile(reportPath, JSON.stringify({
      scenario: spec.name,
      matrix: { agent, language },
      run_id: runId,
      status: scenarioResult.status,
      durationSeconds: scenarioResult.durationSeconds,
      results,
      artifactPaths: scenarioResult.artifactPaths,
    }, null, 2));
    console.log(`${allPassed ? 'PASS' : 'FAIL'} ${spec.name} steps=${results.length}/${spec.steps.length}`);
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

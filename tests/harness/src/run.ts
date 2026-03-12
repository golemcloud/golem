import { parseArgs } from 'node:util';
import * as path from 'node:path';
import * as fs from 'node:fs/promises';
import * as os from 'node:os';
import { ScenarioLoader, ScenarioExecutor, type ScenarioRunResult } from './executor.js';
import { ClaudeAgentDriver } from './driver/claude.js';
import { GeminiAgentDriver } from './driver/gemini.js';
import { OpenCodeAgentDriver } from './driver/opencode.js';
import { SkillWatcher } from './watcher.js';
import { generateHtmlReport, type Summary, type MergedSummary, type HtmlScenarioReport } from './html-report.js';
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

async function runCommand(command: string, args: string[], cwd: string): Promise<{ code: number; output: string }> {
  const { spawn } = await import('node:child_process');
  return new Promise((resolve) => {
    const child = spawn(command, args, {
      cwd,
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    let output = '';
    child.stdout?.on('data', (data: Buffer) => (output += data.toString()));
    child.stderr?.on('data', (data: Buffer) => (output += data.toString()));
    child.on('close', (code: number) => resolve({ code: code ?? 1, output }));
    child.on('error', (err: Error) => resolve({ code: 1, output: err.message }));
  });
}

async function cleanupGolemState(_cwd: string): Promise<void> {
  const isCI = !!process.env['GITHUB_ACTIONS'] || !!process.env['CI'];
  const isTTY = !!process.stdin.isTTY;

  if (!isCI && isTTY) {
    const readline = await import('node:readline');
    const rl = readline.createInterface({ input: process.stdin, output: process.stdout });
    const answer = await new Promise<string>((resolve) => {
      rl.question(chalk.yellow('This will stop the Golem server, wipe all data, and restart it. Continue? [y/N] '), resolve);
    });
    rl.close();
    if (answer.toLowerCase() !== 'y') {
      console.log(chalk.gray('Skipping cleanup'));
      return;
    }
  } else if (!isCI) {
    console.log(chalk.yellow('Non-interactive mode: proceeding with Golem server cleanup'));
  }

  // Stop the running server
  console.log(chalk.gray('Stopping Golem server...'));
  await runCommand('pkill', ['-f', 'golem-server'], _cwd);
  // Give it a moment to shut down
  await new Promise((r) => setTimeout(r, 2000));

  // Clean all server data
  console.log(chalk.gray('Cleaning Golem server data...'));
  const cleanResult = await runCommand('golem', ['server', 'clean', '--yes'], _cwd);
  if (cleanResult.code !== 0) {
    console.warn(chalk.yellow(`Warning: golem server clean failed (exit ${cleanResult.code}): ${cleanResult.output.trim()}`));
  }

  // Restart the server in background
  console.log(chalk.gray('Restarting Golem server...'));
  const { spawn } = await import('node:child_process');
  const serverProcess = spawn('golem', ['server', 'run'], {
    cwd: _cwd,
    stdio: 'ignore',
    detached: true,
  });
  serverProcess.unref();

  // Wait for server to be ready
  const maxWait = 30;
  for (let i = 0; i < maxWait; i++) {
    await new Promise((r) => setTimeout(r, 1000));
    const health = await runCommand('curl', ['-fsS', 'http://localhost:9881/healthcheck'], _cwd);
    if (health.code === 0) {
      console.log(chalk.gray('Golem server is ready'));
      return;
    }
  }
  console.warn(chalk.yellow('Warning: Golem server did not become ready within 30s'));
}

async function mergeReports(reportsDir: string, outputDir: string): Promise<void> {
  await fs.mkdir(outputDir, { recursive: true });

  const files = (await fs.readdir(reportsDir)).filter(f => f === 'summary.json' || f.endsWith('-summary.json'));
  if (files.length === 0) {
    // Try reading all summary.json from subdirectories
    const entries = await fs.readdir(reportsDir, { withFileTypes: true });
    for (const entry of entries) {
      if (entry.isDirectory()) {
        try {
          const summaryPath = path.join(reportsDir, entry.name, 'summary.json');
          await fs.access(summaryPath);
          files.push(path.join(entry.name, 'summary.json'));
        } catch {
          // no summary in this dir
        }
      }
    }
  }

  const summaries: Summary[] = [];
  for (const file of files) {
    const content = await fs.readFile(path.join(reportsDir, file), 'utf8');
    summaries.push(JSON.parse(content) as Summary);
  }

  if (summaries.length === 0) {
    console.error(chalk.red('No summary.json files found in the reports directory'));
    process.exit(1);
  }

  const agents = new Set<string>();
  const languages = new Set<string>();
  const osSet = new Set<string>();
  const heatMap: MergedSummary['heatMap'] = [];

  for (const s of summaries) {
    const agent = s.agent ?? 'unknown';
    const lang = s.language ?? 'unknown';
    const sOs = s.os ?? 'unknown';
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

  const mergedPath = path.join(outputDir, 'merged-summary.json');
  await fs.writeFile(mergedPath, JSON.stringify(merged, null, 2));
  console.log(chalk.green(`Merged summary written to ${mergedPath}`));

  // Generate HTML for merged report
  const htmlContent = generateHtmlReport(merged, []);
  const htmlPath = path.join(outputDir, 'report.html');
  await fs.writeFile(htmlPath, htmlContent);
  console.log(chalk.green(`HTML report written to ${htmlPath}`));
}

async function main() {
  const { values } = parseArgs({
    options: {
      agent: { type: 'string' },
      language: { type: 'string' },
      scenarios: { type: 'string' },
      output: { type: 'string', default: './results' },
      scenario: { type: 'string' },
      timeout: { type: 'string' },
      skills: { type: 'string', default: '../../skills' },
      'resume-from': { type: 'string' },
      workspace: { type: 'string' },
      'no-cleanup': { type: 'boolean', default: false },
      'merge-reports': { type: 'string' },
      help: { type: 'boolean', short: 'h', default: false }
    }
  });

  const {
    agent, language, scenarios, output,
    scenario: scenarioFilter, timeout, skills: skillsDirRel,
    'resume-from': resumeFrom, workspace: workspaceOverride,
    'no-cleanup': noCleanup, 'merge-reports': mergeReportsDir,
    help,
  } = values;

  // Merge-reports mode — standalone, doesn't require --agent/--language/--scenarios
  if (mergeReportsDir) {
    const outputDir = path.resolve(process.cwd(), output!);
    await mergeReports(path.resolve(process.cwd(), mergeReportsDir), outputDir);
    return;
  }

  if (help || !agent || !language || !scenarios) {
    const usage = `
golem-skill-harness — Skill testing harness for Golem coding agents

Usage:
  npx tsx src/run.ts --agent <name> --language <ts|rust> --scenarios <dir> [options]

Required:
  --agent <name>            Agent driver to use (claude-code, gemini, opencode)
  --language <lang>         Language for skill templates (ts, rust)
  --scenarios <dir>         Path to scenario YAML files

Options:
  --scenario <name>         Run only the named scenario
  --output <dir>            Results output directory (default: ./results)
  --timeout <seconds>       Global timeout per scenario in seconds
  --skills <dir>            Path to skills directory (default: ../../skills)
  --resume-from <step-id>   Resume execution from the given step ID
  --workspace <path>        Override workspace directory (implies --no-cleanup)
  --no-cleanup              Skip Golem state cleanup between scenarios
  --merge-reports <dir>     Merge summary.json files from <dir> into aggregated report
  -h, --help                Show this help message
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
  const skipCleanup = noCleanup || !!workspaceOverride;

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
  } else {
    console.error(chalk.red(`Unsupported agent: ${agent}`));
    process.exit(1);
  }

  const watcher = new SkillWatcher(skillsDir);
  const scenarioFiles = (await fs.readdir(scenariosDir)).filter(f => f.endsWith('.yaml') || f.endsWith('.yml'));
  console.log(chalk.gray(`Config: agent=${agent}, language=${language}, scenarios=${scenariosDir}, output=${resultsDir}, timeout=${globalTimeoutSeconds ?? 'default'}`));

  const scenarioReports: ScenarioReport[] = [];
  let skippedCount = 0;
  let hasFailures = false;
  let isFirstScenario = true;

  for (const file of scenarioFiles) {
    const spec = await ScenarioLoader.load(path.join(scenariosDir, file));

    if (scenarioFilter && spec.name !== scenarioFilter) continue;

    // Matrix filtering (#2911)
    if (!ScenarioLoader.matchesMatrix(spec, agent, language)) {
      console.log(chalk.gray(`Skipping scenario "${spec.name}" (matrix mismatch)`));
      skippedCount++;
      continue;
    }

    // Cleanup Golem state between scenarios (#2913)
    if (!isFirstScenario && !skipCleanup) {
      console.log(chalk.gray('Cleaning up Golem state between scenarios...'));
      await cleanupGolemState(process.cwd());
    }
    isFirstScenario = false;

    console.log(chalk.blue(`Running scenario: ${spec.name}`));
    const workspace = workspaceOverride
      ? path.resolve(process.cwd(), workspaceOverride)
      : path.join(process.cwd(), 'workspaces', spec.name.replace(/\s+/g, '-').toLowerCase());
    const executor = new ScenarioExecutor(driver, watcher, workspace, skillsDir, {
      globalTimeoutSeconds,
      resumeFromStepId: resumeFrom,
      skipCleanup,
    });

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

    const summary: Summary = {
      agent,
      language,
      os: os.platform(),
      timestamp: new Date().toISOString(),
      total: totalScenarios,
      passed,
      failed,
      skipped: skippedCount,
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

    // Generate HTML report (#2903)
    const htmlContent = generateHtmlReport(summary, scenarioReports as HtmlScenarioReport[]);
    const htmlPath = path.join(resultsDir, 'report.html');
    await fs.writeFile(htmlPath, htmlContent);

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
    if (skippedCount > 0) {
      console.log(chalk.yellow(`Skipped:  ${skippedCount}`));
    }
    console.log(`Duration: ${totalDuration.toFixed(1)}s`);

    if (worstFailures.length > 0) {
      console.log('');
      console.log(chalk.red('Failures:'));
      for (const f of worstFailures) {
        console.log(chalk.red(`  ${f.scenario}: ${f.error}`));
      }
    }

    console.log(chalk.gray(`Reports: ${summaryPath}, ${path.join(resultsDir, 'report.html')}`));
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

import chalk from "chalk";

// ---------------------------------------------------------------------------
// Centralised logging for the skill-test harness.
//
// Every piece of user-visible output goes through this module so we can
// control formatting, colours, and icons in one place.
// All output is written to **stdout** — stderr of spawned processes is
// distinguished by colour, not by file descriptor.
// ---------------------------------------------------------------------------

// --- Prefixes / helpers ----------------------------------------------------

const GOLEM_PREFIX = chalk.magenta("[golem]");

function prefixed(prefix: string, line: string): void {
  console.log(`${prefix} ${line}`);
}

// --- Golem server output ---------------------------------------------------

export function golemServer(line: string): void {
  prefixed(GOLEM_PREFIX, line);
}

export function golemServerErr(line: string): void {
  prefixed(GOLEM_PREFIX, chalk.gray(line));
}

export function golemServerError(msg: string): void {
  prefixed(GOLEM_PREFIX, chalk.red(`process error: ${msg}`));
}

export function golemServerExit(code: number): void {
  prefixed(GOLEM_PREFIX, chalk.red(`exited with code ${code}`));
}

// --- Agent driver output ---------------------------------------------------

export function driver(prefix: string, line: string): void {
  prefixed(prefix, line);
}

export function driverErr(prefix: string, line: string): void {
  prefixed(prefix, chalk.gray(line));
}

// --- Agent driver events (Amp-style) ---------------------------------------

export function driverSession(prefix: string, sessionId: string): void {
  prefixed(prefix, `${chalk.cyan("session")} ${chalk.gray(sessionId)}`);
}

export function driverCwd(prefix: string, cwd: string): void {
  prefixed(prefix, `${chalk.cyan("cwd")} ${chalk.gray(cwd)}`);
}

export function driverTools(prefix: string, tools: string[]): void {
  const preview = tools.slice(0, 8).join(", ");
  const suffix = tools.length > 8 ? ", ..." : "";
  prefixed(
    prefix,
    `${chalk.cyan("tools")} ${chalk.gray(`(${tools.length})`)} ${chalk.gray(preview + suffix)}`,
  );
}

export function driverMcp(
  prefix: string,
  name: string,
  status: string,
): void {
  const statusColor = status === "connected" ? chalk.green : chalk.yellow;
  prefixed(prefix, `${chalk.cyan("mcp")} ${chalk.white(name)} ${statusColor(status)}`);
}

export function driverToolUse(
  prefix: string,
  toolName: string,
  input?: Record<string, unknown>,
): void {
  const inputStr =
    input && Object.keys(input).length > 0
      ? " " + chalk.gray(JSON.stringify(input))
      : "";
  prefixed(prefix, `${chalk.yellow("▶")} ${chalk.yellow(toolName)}${inputStr}`);
}

export function driverSuccess(
  prefix: string,
  durationStr: string,
  extra?: string,
): void {
  const parts = [chalk.green("✓ done"), chalk.gray(durationStr)];
  if (extra) parts.push(chalk.gray(extra));
  prefixed(prefix, parts.join(" "));
}

export function driverError(prefix: string, msg: string, durationStr?: string): void {
  const parts = [chalk.red("✗ error")];
  if (durationStr) parts.push(chalk.gray(durationStr));
  prefixed(prefix, parts.join(" "));
  if (msg) prefixed(prefix, chalk.red(msg));
}

export function driverStreamEnd(prefix: string): void {
  prefixed(prefix, chalk.red("✗ stream ended without result"));
}

export function driverTimeout(prefix: string, seconds: number): void {
  prefixed(prefix, chalk.red(`✗ timed out after ${seconds}s`));
}

export function driverNotInstalled(prefix: string): void {
  prefixed(prefix, chalk.red("✗ Amp CLI not installed"));
}

export function driverAuthFailed(prefix: string): void {
  prefixed(prefix, chalk.red("✗ authentication failed"));
}

export function driverFatal(prefix: string, msg: string): void {
  prefixed(prefix, chalk.red(`✗ ${msg}`));
}

// --- Scenario / step lifecycle ---------------------------------------------

export function scenarioSkip(name: string): void {
  console.log(`Scenario ${name}: skipped (skip_if condition met)`);
}

export function stepStart(label: string, timeout: number): void {
  console.log(`Step ${label}: starting (timeout=${timeout}s)`);
}

export function stepSkip(label: string, reason: string): void {
  console.log(`Step ${label}: skipped (${reason})`);
}

export function stepRetry(
  label: string,
  attempt: number,
  maxAttempts: number,
  delay: number,
): void {
  console.log(
    `Step ${label}: retry attempt ${attempt}/${maxAttempts} (delay=${delay}s)`,
  );
}

export function stepAction(label: string, description: string): void {
  console.log(`Step ${label}: ${description}`);
}

export function stepSkillDetected(
  label: string,
  method: "fswatch" | "atime",
  skillName: string,
  path: string,
): void {
  console.log(`Step ${label}: ${method} detected "${skillName}" via ${path}`);
}

export function stepActivatedSkills(label: string, skills: string[]): void {
  console.log(`Step ${label}: activated skills [${skills.join(", ")}]`);
}

// --- Scenario results ------------------------------------------------------

export function scenarioPass(name: string): void {
  console.log(chalk.green(`Scenario ${name} PASSED`));
}

export function scenarioFail(name: string): void {
  console.log(chalk.red(`Scenario ${name} FAILED`));
}

export function scenarioFailedStep(stepName: string, error: string): void {
  console.log(chalk.red(`  Step failed: ${stepName}`));
  console.log(chalk.red(`  Error: ${error}`));
}

export function scenarioFailureClassification(
  category: string,
  guidance: string,
): void {
  console.log(chalk.yellow(`  [${category}] ${guidance}`));
}

export function scenarioResultLine(
  passed: boolean,
  name: string,
  stepsCompleted: number,
  stepsTotal: number,
): void {
  console.log(
    `${passed ? "PASS" : "FAIL"} ${name} steps=${stepsCompleted}/${stepsTotal}`,
  );
}

// --- Run-level info --------------------------------------------------------

export function info(msg: string): void {
  console.log(chalk.cyan(msg));
}

export function success(msg: string): void {
  console.log(chalk.green(msg));
}

export function warn(msg: string): void {
  console.log(chalk.yellow(msg));
}

export function error(msg: string): void {
  console.log(chalk.red(msg));
}

export function fatal(msg: string): void {
  console.log(chalk.red(msg));
}

export function dim(msg: string): void {
  console.log(chalk.gray(msg));
}

export function bold(msg: string): void {
  console.log(chalk.bold(msg));
}

export function heading(msg: string): void {
  console.log(chalk.blue(msg));
}

export function usage(text: string): void {
  console.log(text);
}

export function plain(msg: string): void {
  console.log(msg);
}

export function blank(): void {
  console.log("");
}

// --- Dry run ---------------------------------------------------------------

export function dryRunStepLine(label: string, preview: string): void {
  console.log(`  [${label}] ${preview}`);
}

export function dryRunStepDetail(detail: string): void {
  console.log(`    ${detail}`);
}

// --- Test summary ----------------------------------------------------------

export function summaryLine(label: string, value: string | number, color?: "green" | "red"): void {
  const formatted = color === "green" ? chalk.green(`${label}${value}`)
    : color === "red" ? chalk.red(`${label}${value}`)
    : `${label}${value}`;
  console.log(formatted);
}

export function summaryFailure(scenario: string, errorMsg: string): void {
  console.log(chalk.red(`  ${scenario}: ${errorMsg}`));
}

export function summaryGuidance(guidance: string): void {
  console.log(chalk.yellow(`    ${guidance}`));
}

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

type Colorizer = (text: string) => string;

const TOOL_INLINE_VALUE_LIMIT = 200;

const DRIVER_TAGS = ["amp", "claude-code", "codex", "opencode"] as const;
const TAG_WIDTH = Math.max(
  ...["golem", "scenario", "step", "run", "dry-run", "summary", ...DRIVER_TAGS].map(
    (tag) => `[${tag}]`.length,
  ),
);

function renderTag(tag: string, color: Colorizer): string {
  return color(`[${tag}]`.padEnd(TAG_WIDTH));
}

function emit(tag: string, color: Colorizer, text: string, lineColor?: Colorizer): void {
  const prefix = renderTag(tag, color);
  const lines = text.split(/\r?\n/);

  for (const line of lines) {
    const renderedLine = lineColor ? lineColor(line) : line;
    console.log(renderedLine.length > 0 ? `${prefix} ${renderedLine}` : prefix);
  }
}

function golemLine(line: string): void {
  emit("golem", chalk.magenta, line);
}

function driverLine(driver: string, line: string, lineColor?: Colorizer): void {
  emit(driver, chalk.magenta, line, lineColor);
}

function scenarioLine(name: string, line: string): void {
  emit("scenario", chalk.blue, `${chalk.bold(name)} ${line}`);
}

function scenarioDetail(line: string, lineColor?: Colorizer): void {
  emit("scenario", chalk.blue, line, lineColor);
}

export function stepLine(label: string, line: string): void {
  emit("step", chalk.cyan, `${chalk.bold(label)} ${line}`);
}

function runLine(line: string, lineColor?: Colorizer): void {
  emit("run", chalk.cyan, line, lineColor);
}

function dryRunLine(line: string, lineColor?: Colorizer): void {
  emit("dry-run", chalk.blue, line, lineColor);
}

function summaryTagged(line: string, lineColor?: Colorizer): void {
  emit("summary", chalk.green, line, lineColor);
}

function formatStepAction(description: string): string {
  let match = /^verifying (\d+) expected (file|files)$/.exec(description);
  if (match) {
    const [, count, label] = match;
    return `${chalk.cyan("• verify")} ${chalk.white(`expected ${label}`)} ${chalk.gray(`count=${count}`)}`;
  }

  match = /^expected file exists: (.+)$/.exec(description);
  if (match) {
    const [, relPath] = match;
    return `${chalk.green("✓ file")} ${chalk.white(relPath)}`;
  }

  match = /^expected (file|files) verified$/.exec(description);
  if (match) {
    const [, label] = match;
    return `${chalk.green("✓ verify")} ${chalk.white(`expected ${label} verified`)}`;
  }

  match = /^expected (file|files) verification failed \((.+)\)$/.exec(description);
  if (match) {
    const [, label, detail] = match;
    return `${chalk.red("✗ verify")} ${chalk.white(`expected ${label} verification failed`)} ${chalk.gray(detail)}`;
  }

  match = /^running implicit golem build before deploy in (.+)$/.exec(description);
  if (match) {
    const [, cwd] = match;
    return `${chalk.yellow("▶ golem build")} ${chalk.gray("implicit-before-deploy")} ${chalk.gray(`cwd=${cwd}`)}`;
  }

  match = /^running golem build in (.+)$/.exec(description);
  if (match) {
    const [, cwd] = match;
    return `${chalk.yellow("▶ golem build")} ${chalk.gray(`cwd=${cwd}`)}`;
  }

  match = /^running golem deploy in (.+)$/.exec(description);
  if (match) {
    const [, cwd] = match;
    return `${chalk.yellow("▶ golem deploy")} ${chalk.gray(`cwd=${cwd}`)}`;
  }

  match = /^sleeping for (.+)$/.exec(description);
  if (match) {
    const [, duration] = match;
    return `${chalk.cyan("• sleep")} ${chalk.gray(duration)}`;
  }

  match = /^creating agent "(.+)"$/.exec(description);
  if (match) {
    const [, name] = match;
    return `${chalk.cyan("• create agent")} ${chalk.white(name)}`;
  }

  match = /^deleting agent "(.+)"$/.exec(description);
  if (match) {
    const [, name] = match;
    return `${chalk.cyan("• delete agent")} ${chalk.white(name)}`;
  }

  match = /^running shell command(?::| )"?(.+?)"?$/.exec(description);
  if (match) {
    const [, command] = match;
    return `${chalk.yellow("▶ shell")} ${chalk.white(command)}`;
  }

  match = /^triggering (.+)$/.exec(description);
  if (match) {
    const [, target] = match;
    return `${chalk.yellow("▶ trigger")} ${chalk.white(target)}`;
  }

  match = /^invoking \(json\) (.+)$/.exec(description);
  if (match) {
    const [, target] = match;
    return `${chalk.yellow("▶ invoke --json")} ${chalk.white(target)}`;
  }

  match = /^invoking (.+)$/.exec(description);
  if (match) {
    const [, target] = match;
    return `${chalk.yellow("▶ invoke")} ${chalk.white(target)}`;
  }

  match = /^HTTP ([A-Z]+) (.+)$/.exec(description);
  if (match) {
    const [, method, url] = match;
    return `${chalk.yellow("▶ http")} ${chalk.white(`${method} ${url}`)}`;
  }

  return `${chalk.cyan("•")} ${description}`;
}

// --- Golem server output ---------------------------------------------------

export function golemServer(line: string): void {
  golemLine(line);
}

export function golemServerErr(line: string): void {
  golemLine(chalk.gray(line));
}

export function golemServerError(msg: string): void {
  golemLine(chalk.red(`process error: ${msg}`));
}

export function golemServerExit(code: number): void {
  golemLine(chalk.red(`exited with code ${code}`));
}

// --- Agent driver output ---------------------------------------------------

export function driver(prefix: string, line: string): void {
  driverLine(prefix, line);
}

export function driverErr(prefix: string, line: string): void {
  driverLine(prefix, line, chalk.gray);
}

// --- Agent driver events (Amp-style) ---------------------------------------

export function driverSession(prefix: string, sessionId: string): void {
  driverLine(prefix, `${chalk.cyan("session")} ${chalk.gray(sessionId)}`);
}

export function driverCwd(prefix: string, cwd: string): void {
  driverLine(prefix, `${chalk.cyan("cwd")} ${chalk.gray(cwd)}`);
}

export function driverTools(prefix: string, tools: string[]): void {
  const preview = tools.slice(0, 8).join(", ");
  const suffix = tools.length > 8 ? ", ..." : "";
  driverLine(
    prefix,
    `${chalk.cyan("tools")} ${chalk.gray(`(${tools.length})`)} ${chalk.gray(preview + suffix)}`,
  );
}

export function driverMcp(prefix: string, name: string, status: string): void {
  const statusColor = status === "connected" ? chalk.green : chalk.yellow;
  driverLine(prefix, `${chalk.cyan("mcp")} ${chalk.white(name)} ${statusColor(status)}`);
}

export function driverToolUse(
  prefix: string,
  toolName: string,
  input?: Record<string, unknown>,
): void {
  const { summary, details } = formatToolLog(toolName, input);
  driverLine(prefix, `${chalk.yellow("▶")} ${chalk.yellow(toolName)}${summary}`);
  for (const line of details) {
    driverLine(prefix, `${chalk.gray("│")} ${line}`);
  }
}

function formatToolLog(
  toolName: string,
  input?: Record<string, unknown>,
): { summary: string; details: string[] } {
  if (!input || Object.keys(input).length === 0) {
    return { summary: "", details: [] };
  }

  const filePath = pickString(input, [
    "path",
    "file_path",
    "filePath",
    "target_file",
    "targetFile",
    "newPath",
    "new_path",
  ]);
  const patchText = pickString(input, ["patchText", "patch", "diff"]);
  if (patchText !== undefined) {
    return {
      summary: filePath ? ` ${chalk.gray(filePath)}` : "",
      details: limitDetailLines(patchText.split(/\r?\n/)),
    };
  }

  const content = pickString(input, ["content", "file_text", "fileText"]);
  const oldText = pickString(input, ["old_str", "oldText", "old_text"]);
  const newText = pickString(input, ["new_str", "newText", "new_text"]);

  if (filePath && (content !== undefined || oldText !== undefined || newText !== undefined)) {
    const rest = omitKeys(input, [
      "path",
      "file_path",
      "filePath",
      "target_file",
      "targetFile",
      "newPath",
      "new_path",
      "content",
      "file_text",
      "fileText",
      "old_str",
      "oldText",
      "old_text",
      "new_str",
      "newText",
      "new_text",
    ]);

    const details = [
      ...formatObjectDetails(rest),
      ...(content !== undefined ? formatNamedBlock("content", content) : []),
      ...(oldText !== undefined ? formatNamedBlock("old", oldText) : []),
      ...(newText !== undefined ? formatNamedBlock("new", newText) : []),
    ];

    return {
      summary: ` ${chalk.gray(filePath)}`,
      details: limitDetailLines(details),
    };
  }

  if (filePath) {
    const rest = omitKeys(input, [
      "path",
      "file_path",
      "filePath",
      "target_file",
      "targetFile",
      "newPath",
      "new_path",
    ]);
    return {
      summary: ` ${chalk.gray(filePath)}`,
      details: formatObjectDetails(rest),
    };
  }

  const command = pickString(input, ["command", "cmd"]);
  if (command !== undefined) {
    const rest = omitKeys(input, ["command", "cmd"]);
    const commandSummary =
      command.length <= TOOL_INLINE_VALUE_LIMIT ? ` ${chalk.gray(command)}` : "";
    return {
      summary: commandSummary,
      details:
        command.length <= TOOL_INLINE_VALUE_LIMIT
          ? formatObjectDetails(rest)
          : limitDetailLines([
              ...formatNamedBlock("command", command),
              ...formatObjectDetails(rest),
            ]),
    };
  }

  const pretty = JSON.stringify(input, null, 2);
  if (pretty.length <= TOOL_INLINE_VALUE_LIMIT && !pretty.includes("\n")) {
    return { summary: ` ${chalk.gray(pretty)}`, details: [] };
  }

  return {
    summary: "",
    details: limitDetailLines(pretty.split(/\r?\n/)).map((line) => chalk.gray(line)),
  };
}

function limitDetailLines(lines: string[]): string[] {
  return lines;
}

function pickString(input: Record<string, unknown>, keys: string[]): string | undefined {
  for (const key of keys) {
    const value = input[key];
    if (typeof value === "string") {
      return value;
    }
  }
  return undefined;
}

function omitKeys(input: Record<string, unknown>, keys: string[]): Record<string, unknown> {
  const result = { ...input };
  for (const key of keys) {
    delete result[key];
  }
  return result;
}

function formatObjectDetails(input: Record<string, unknown>): string[] {
  if (Object.keys(input).length === 0) {
    return [];
  }
  return limitDetailLines(JSON.stringify(input, null, 2).split(/\r?\n/)).map((line) =>
    chalk.gray(line),
  );
}

function formatNamedBlock(label: string, value: string): string[] {
  return [chalk.gray(`${label}:`), ...value.split(/\r?\n/)];
}

export function driverSuccess(prefix: string, durationStr: string, extra?: string): void {
  const parts = [chalk.green("✓ done"), chalk.gray(durationStr)];
  if (extra) parts.push(chalk.gray(extra));
  driverLine(prefix, parts.join(" "));
}

export function driverError(prefix: string, msg: string, durationStr?: string): void {
  const parts = [chalk.red("✗ error")];
  if (durationStr) parts.push(chalk.gray(durationStr));
  driverLine(prefix, parts.join(" "));
  if (msg) driverLine(prefix, msg, chalk.red);
}

export function driverStreamEnd(prefix: string): void {
  driverLine(prefix, chalk.red("✗ stream ended without result"));
}

export function driverTimeout(prefix: string, seconds: number): void {
  driverLine(prefix, chalk.red(`✗ timed out after ${seconds}s`));
}

export function driverIdleTimeout(prefix: string, seconds: number): void {
  driverLine(prefix, chalk.red(`✗ idle timeout — no output for ${seconds}s`));
}

export function driverNotInstalled(prefix: string): void {
  driverLine(prefix, chalk.red("✗ Amp CLI not installed"));
}

export function driverAuthFailed(prefix: string): void {
  driverLine(prefix, chalk.red("✗ authentication failed"));
}

export function driverFatal(prefix: string, msg: string): void {
  driverLine(prefix, chalk.red(`✗ ${msg}`));
}

export function driverHeartbeat(prefix: string, elapsedSeconds: number): void {
  driverLine(prefix, chalk.gray(`⏳ waiting for agent response… (${elapsedSeconds}s elapsed)`));
}

// --- Scenario / step lifecycle ---------------------------------------------

export function scenarioSkip(name: string): void {
  scenarioLine(name, `${chalk.yellow("↷ skipped")} ${chalk.gray("skip_if condition met")}`);
}

export function stepStart(label: string, timeout: number): void {
  stepLine(label, `${chalk.blue("▶ start")} ${chalk.gray(`timeout=${timeout}s`)}`);
}

export function stepSkip(label: string, reason: string): void {
  stepLine(label, `${chalk.yellow("↷ skipped")} ${chalk.gray(reason)}`);
}

export function stepRetry(
  label: string,
  attempt: number,
  maxAttempts: number,
  delay: number,
): void {
  stepLine(
    label,
    `${chalk.yellow("↻ retry")} ${chalk.white(`${attempt}/${maxAttempts}`)} ${chalk.gray(`delay=${delay}s`)}`,
  );
}

export function stepAction(label: string, description: string): void {
  stepLine(label, formatStepAction(description));
}

export function stepOutput(label: string, stream: "stdout" | "stderr", text: string): void {
  const prefix = stream === "stderr" ? chalk.red("err│") : chalk.gray("out│");
  for (const line of text.split(/\r?\n/)) {
    stepLine(label, `${prefix} ${line}`);
  }
}

export function stepPrompt(
  label: string,
  prompt: string,
  kind: "initial" | "followup" = "initial",
): void {
  stepLine(
    label,
    kind === "followup"
      ? `${chalk.yellow("▶ prompt")} ${chalk.gray("followup")}`
      : chalk.yellow("▶ prompt"),
  );

  for (const line of prompt.split(/\r?\n/)) {
    stepLine(label, `${chalk.gray("│")} ${line}`);
  }
}

export function stepSkillDetected(
  label: string,
  method: "fswatch" | "atime" | "driver",
  skillName: string,
  path: string,
): void {
  stepLine(
    label,
    `${chalk.magenta("◆ skill")} ${chalk.green(skillName)} ${chalk.gray(`detected via ${method}`)} ${chalk.dim(path)}`,
  );
}

export function stepActivatedSkills(label: string, skills: string[]): void {
  if (skills.length === 0) {
    stepLine(label, `${chalk.yellow("• skills")} ${chalk.gray("none activated")}`);
    return;
  }

  stepLine(
    label,
    `${chalk.green("✓ skills")} ${chalk.gray(`count=${skills.length}`)} ${chalk.white(skills.join(", "))}`,
  );
}

// --- CLI command output ----------------------------------------------------

export function cliOutput(label: string, command: string, output: string): void {
  if (!output.trim()) return;
  stepLine(label, `${chalk.gray(`[${command}]`)}`);
  for (const line of output.split(/\r?\n/)) {
    if (line.trim()) stepLine(label, `${chalk.gray("│")} ${chalk.gray(line)}`);
  }
}

export function invokeResult(label: string, target: string, stdout: string): void {
  stepLine(label, `${chalk.green("✓ invoke")} ${chalk.white(target)} ${chalk.gray("result:")}`);
  for (const line of stdout.split(/\r?\n/)) {
    if (line.trim()) stepLine(label, `${chalk.gray("│")} ${chalk.white(line)}`);
  }
}

export function httpResponse(label: string, status: number, body: string): void {
  stepLine(label, `${chalk.green("✓ http")} ${chalk.gray(`status=${status}`)}`);
  if (!body.trim()) {
    stepLine(label, `${chalk.gray("│")} ${chalk.gray("<empty body>")}`);
    return;
  }

  for (const line of body.split(/\r?\n/)) {
    stepLine(label, `${chalk.gray("│")} ${chalk.white(line)}`);
  }
}

export function httpFailure(label: string, message: string): void {
  stepLine(label, `${chalk.red("✗ http")} ${chalk.red(message)}`);
}

export function mcpResponse(label: string, method: string, status: number, body: string): void {
  stepLine(
    label,
    `${chalk.green("✓ mcp")} ${chalk.cyan(method)} ${chalk.gray(`status=${status}`)}`,
  );
  if (!body.trim()) {
    stepLine(label, `${chalk.gray("│")} ${chalk.gray("<empty body>")}`);
    return;
  }

  for (const line of body.split(/\r?\n/)) {
    stepLine(label, `${chalk.gray("│")} ${chalk.white(line)}`);
  }
}

export function mcpFailure(label: string, message: string): void {
  stepLine(label, `${chalk.red("✗ mcp")} ${chalk.red(message)}`);
}

export function assertionPassed(label: string, assertion: string, message: string): void {
  stepLine(label, `${chalk.green("✓ assertion")} ${chalk.white(assertion)} ${chalk.gray(message)}`);
}

export function assertionFailed(label: string, assertion: string, message: string): void {
  stepLine(label, `${chalk.red("✗ assertion")} ${chalk.white(assertion)} ${chalk.gray(message)}`);
}

// --- Scenario results ------------------------------------------------------

export function scenarioPass(name: string): void {
  scenarioLine(name, chalk.green("✓ passed"));
}

export function scenarioFail(name: string): void {
  scenarioLine(name, chalk.red("✗ failed"));
}

export function scenarioRetry(
  name: string,
  attempt: number,
  maxAttempts: number,
  reason: string,
): void {
  scenarioLine(
    name,
    `${chalk.yellow("↻ retry")} ${chalk.white(`${attempt}/${maxAttempts}`)} ${chalk.gray(reason)}`,
  );
}

export function scenarioFailedStep(stepName: string, error: string): void {
  scenarioDetail(`step: ${stepName}`, chalk.red);
  scenarioDetail(`error: ${error}`, chalk.red);
}

export function scenarioFailureClassification(category: string, guidance: string): void {
  scenarioDetail(`[${category}] ${guidance}`, chalk.yellow);
}

export function scenarioResultLine(
  passed: boolean,
  name: string,
  stepsCompleted: number,
  stepsTotal: number,
): void {
  scenarioDetail(
    `${passed ? chalk.green("PASS") : chalk.red("FAIL")} ${chalk.bold(name)} ${chalk.gray(`steps=${stepsCompleted}/${stepsTotal}`)}`,
  );
}

// --- Run-level info --------------------------------------------------------

export function info(msg: string): void {
  runLine(msg, chalk.cyan);
}

export function success(msg: string): void {
  runLine(msg, chalk.green);
}

export function warn(msg: string): void {
  runLine(msg, chalk.yellow);
}

export function error(msg: string): void {
  runLine(msg, chalk.red);
}

export function fatal(msg: string): void {
  runLine(msg, chalk.red);
}

export function dim(msg: string): void {
  runLine(msg, chalk.gray);
}

export function bold(msg: string): void {
  runLine(msg, chalk.bold);
}

export function heading(msg: string): void {
  runLine(msg, chalk.blue);
}

export function scenarioSeparator(completed: number, total: number, nextName: string): void {
  const pct = total > 0 ? Math.round((completed / total) * 100) : 0;
  const bar = "═".repeat(72);
  runLine("");
  runLine(bar, chalk.cyan);
  runLine(`  ${completed} of ${total} completed (${pct}%)  ▸  next: ${nextName}`, chalk.cyan.bold);
  runLine(bar, chalk.cyan);
  runLine("");
}

export function usage(text: string): void {
  runLine(text);
}

export function plain(msg: string): void {
  runLine(msg);
}

export function blank(): void {
  runLine("");
}

// --- Dry run ---------------------------------------------------------------

export function dryRunStepLine(label: string, preview: string): void {
  dryRunLine(`${chalk.bold(label)} ${preview}`);
}

export function dryRunStepDetail(detail: string): void {
  dryRunLine(detail, chalk.gray);
}

// --- Test summary ----------------------------------------------------------

export function summaryLine(label: string, value: string | number, color?: "green" | "red"): void {
  const formatted =
    color === "green"
      ? chalk.green(`${label}${value}`)
      : color === "red"
        ? chalk.red(`${label}${value}`)
        : `${label}${value}`;
  summaryTagged(formatted);
}

export function summaryFailure(scenario: string, errorMsg: string): void {
  summaryTagged(`${scenario}: ${errorMsg}`, chalk.red);
}

export function summaryGuidance(guidance: string): void {
  summaryTagged(guidance, chalk.yellow);
}

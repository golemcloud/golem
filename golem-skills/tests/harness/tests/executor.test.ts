import { afterEach, describe, it } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import {
  extractInvokeJsonResult,
  resolveGolemCommandEnv,
  ScenarioExecutor,
  type ScenarioExecutorOptions,
  type ScenarioSpec,
} from "../src/executor.js";
import { SkillWatcher, type WatcherEvent } from "../src/watcher.js";
import type { AgentDriver, AgentResult, DriverTimeoutOptions } from "../src/driver/base.js";

describe("extractInvokeJsonResult", () => {
  it("decodes nested records and schema composites", () => {
    const result = extractInvokeJsonResult(
      JSON.stringify({
        $type: "agent.invoke",
        resultJson: {
          graph: {
            root: {
              kind: "record",
              value: {
                fields: [
                  {
                    name: "items",
                    body: {
                      kind: "list",
                      value: {
                        element: {
                          kind: "record",
                          value: {
                            fields: [
                              { name: "name", body: { kind: "string", value: {} } },
                              {
                                name: "score",
                                body: {
                                  kind: "option",
                                  value: { inner: { kind: "f64", value: {} } },
                                },
                              },
                            ],
                          },
                        },
                      },
                    },
                  },
                ],
              },
            },
          },
          value: {
            kind: "record",
            value: {
              fields: [
                {
                  kind: "list",
                  value: {
                    elements: [
                      {
                        kind: "record",
                        value: {
                          fields: [
                            { kind: "string", value: "first" },
                            { kind: "option", value: { inner: { kind: "f64", value: 2.5 } } },
                          ],
                        },
                      },
                      {
                        kind: "record",
                        value: {
                          fields: [
                            { kind: "string", value: "second" },
                            { kind: "option", value: { inner: null } },
                          ],
                        },
                      },
                    ],
                  },
                },
              ],
            },
          },
        },
      }),
    );

    assert.deepEqual(result, {
      items: [
        { name: "first", score: 2.5 },
        { name: "second", score: null },
      ],
    });
  });

  it("preserves scalar edge values and resolves enum and flags names", () => {
    const result = extractInvokeJsonResult(
      JSON.stringify({
        $type: "agent.invoke",
        resultJson: {
          graph: {
            root: {
              kind: "tuple",
              value: {
                elements: [
                  { kind: "bool", value: {} },
                  { kind: "s64", value: {} },
                  { kind: "string", value: {} },
                  { kind: "char", value: {} },
                  { kind: "enum", value: { cases: ["idle", "ready"] } },
                  { kind: "flags", value: { flags: ["read", "write", "admin"] } },
                ],
              },
            },
          },
          value: {
            kind: "tuple",
            value: {
              elements: [
                { kind: "bool", value: false },
                { kind: "s64", value: 0 },
                { kind: "string", value: "" },
                { kind: "char", value: "🦀" },
                { kind: "enum", value: { case: 1 } },
                { kind: "flags", value: { bits: [true, false, true] } },
              ],
            },
          },
        },
      }),
    );

    assert.deepEqual(result, [false, 0, "", "🦀", "ready", ["read", "admin"]]);
  });

  it("decodes binary, duration, and quantity values from the current schema JSON", () => {
    const result = extractInvokeJsonResult(
      JSON.stringify({
        $type: "agent.invoke",
        resultJson: {
          graph: {
            root: {
              kind: "tuple",
              value: {
                elements: [
                  { kind: "binary", value: {} },
                  { kind: "duration", value: {} },
                  { kind: "quantity", value: { spec: { baseUnit: "kg" } } },
                ],
              },
            },
          },
          value: {
            kind: "tuple",
            value: {
              elements: [
                {
                  kind: "binary",
                  value: { bytes: [0, 127, 255, 251], mimeType: "application/octet-stream" },
                },
                { kind: "duration", value: { nanoseconds: -93_784_000_000_123 } },
                { kind: "quantity", value: { mantissa: 15, scale: 1, unit: "kg" } },
              ],
            },
          },
        },
      }),
    );

    assert.deepEqual(result, [
      { bytes: "AH__-w", mimeType: "application/octet-stream" },
      "-P1DT2H3M4.000000123S",
      { mantissa: 15, scale: 1, unit: "kg" },
    ]);
  });

  it("renders zero, whole-day and fractional-second durations canonically", () => {
    for (const [nanoseconds, expected] of [
      [0, "PT0S"],
      [86_400_000_000_000, "P1D"],
      [1_250_000_000, "PT1.25S"],
    ]) {
      assert.equal(
        extractInvokeJsonResult(
          JSON.stringify({
            $type: "agent.invoke",
            resultJson: {
              graph: { root: { kind: "duration", value: {} } },
              value: { kind: "duration", value: { nanoseconds } },
            },
          }),
        ),
        expected,
      );
    }
  });

  it("preserves raw i64 duration tokens beyond JavaScript number precision", () => {
    for (const [token, expected] of [
      ["9007199254740993", "P104DT5H59M59.254740993S"],
      ["-9007199254740993", "-P104DT5H59M59.254740993S"],
      ["-9223372036854775808", "-P106751DT23H47M16.854775808S"],
    ]) {
      assert.equal(
        extractInvokeJsonResult(
          `{"$type":"agent.invoke","resultJson":{"graph":{"root":{"kind":"duration","value":{}}},"value":{"kind":"duration","value":{"nanoseconds":${token}}}}}`,
        ),
        expected,
      );
    }
    const value = { kind: "duration", value: { nanoseconds: 12 } };
    assert.deepEqual(
      extractInvokeJsonResult(
        JSON.stringify({
          $type: "agent.invoke-session",
          kind: "result",
          value,
        }),
      ),
      value,
    );
  });

  it("allows finite recursive values while rejecting ref-only cycles", () => {
    const ref = { kind: "ref", value: { id: "node" } };
    const graph = {
      root: ref,
      defs: [
        {
          id: "node",
          body: {
            kind: "record",
            value: {
              fields: [
                { name: "label", body: { kind: "string", value: {} } },
                { name: "next", body: { kind: "option", value: { inner: ref } } },
              ],
            },
          },
        },
      ],
    };
    const value = {
      kind: "record",
      value: {
        fields: [
          { kind: "string", value: "first" },
          {
            kind: "option",
            value: {
              inner: {
                kind: "record",
                value: {
                  fields: [
                    { kind: "string", value: "second" },
                    { kind: "option", value: { inner: null } },
                  ],
                },
              },
            },
          },
        ],
      },
    };
    assert.deepEqual(
      extractInvokeJsonResult(
        JSON.stringify({
          $type: "agent.invoke",
          resultJson: { graph, value },
        }),
      ),
      { label: "first", next: { label: "second", next: null } },
    );
    assert.equal(
      extractInvokeJsonResult(
        JSON.stringify({
          $type: "agent.invoke",
          resultJson: { graph: { root: ref, defs: [{ id: "node", body: ref }] }, value },
        }),
      ),
      undefined,
    );
  });

  it("keeps current invocation-session result documents unchanged", () => {
    const value = { count: 2, active: true };
    assert.deepEqual(
      extractInvokeJsonResult(
        [
          { $type: "agent.invoke-session", kind: "accepted" },
          { $type: "agent.invoke-session", kind: "result", value },
          { $type: "agent.invoke-session", kind: "finished", outcome: "success" },
        ]
          .map((document) => JSON.stringify(document))
          .join("\n"),
      ),
      value,
    );
  });
});

describe("resolveGolemCommandEnv", () => {
  it("uses scenario ports when there is no run override", () => {
    const env = resolveGolemCommandEnv(
      {
        name: "ports",
        settings: { golem_server: { router_port: 9890, custom_request_port: 9020 } },
        steps: [{ tag: "sleep", sleep: 0 }],
      },
      {},
    );
    assert.equal(env.GOLEM_ROUTER_PORT, "9890");
    assert.equal(env.GOLEM_CUSTOM_REQUEST_PORT, "9020");
    assert.equal(env.GOLEM_BUILTIN_LOCAL_URL, "http://localhost:9890");
  });

  it("propagates run-level router, custom request, and MCP ports", () => {
    const env = resolveGolemCommandEnv(
      {
        name: "ports",
        settings: { golem_server: { router_port: 9890, custom_request_port: 9020 } },
        steps: [{ tag: "sleep", sleep: 0 }],
      },
      {
        GOLEM_ROUTER_PORT: "9893",
        GOLEM_CUSTOM_REQUEST_PORT: "9028",
        GOLEM_MCP_PORT: "9029",
      },
    );
    assert.equal(env.GOLEM_ROUTER_PORT, "9893");
    assert.equal(env.GOLEM_CUSTOM_REQUEST_PORT, "9028");
    assert.equal(env.GOLEM_MCP_PORT, "9029");
    assert.equal(env.GOLEM_BUILTIN_LOCAL_URL, "http://localhost:9893");
  });
});

type StepLike = {
  expectedSkills?: string[];
  strictSkillMatch?: boolean;
  allowedExtraSkills?: string[];
};
type ExecutorWithPrivate = {
  assertSkillActivation(label: string, step: StepLike, activated: string[]): string | undefined;
  workspace: string;
  findGolemProjectDir(): Promise<string>;
  runLocalCommand(
    command: string,
    args: string[],
    timeoutSeconds: number,
    cwd: string,
    extraEnv?: Record<string, string>,
  ): Promise<{ success: boolean; output: string }>;
  executeVerification(
    stepLabel: string,
    verify: { build?: boolean; deploy?: boolean; expectedFiles?: string[] },
    commandEnv: Record<string, string>,
    currentSuccess: boolean,
    fail: (msg: string) => void,
  ): Promise<void>;
};

function assertSkillActivation(step: StepLike, activatedSkills: string[]): string | undefined {
  const executor = Object.create(ScenarioExecutor.prototype) as unknown as ExecutorWithPrivate;
  return executor.assertSkillActivation("test", step, activatedSkills);
}

const tempDirs: string[] = [];

async function createTempHarnessDirs(): Promise<{
  workspace: string;
  bootstrapSkillSourceDirs: string[];
}> {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "executor-test-"));
  tempDirs.push(root);
  const workspace = path.join(root, "workspace");
  const bootstrapSkillSourceDir = path.join(root, "bootstrap-skill");
  await fs.mkdir(workspace, { recursive: true });
  await fs.mkdir(bootstrapSkillSourceDir, { recursive: true });
  await fs.writeFile(path.join(bootstrapSkillSourceDir, "SKILL.md"), "bootstrap", "utf8");
  return { workspace, bootstrapSkillSourceDirs: [bootstrapSkillSourceDir] };
}

function createExecutor(
  driver: AgentDriver,
  watcher: SkillWatcher,
  workspace: string,
  bootstrapSkillSourceDirs: string[],
  options?: ScenarioExecutorOptions,
): ScenarioExecutor {
  const executor = new ScenarioExecutor(
    driver,
    watcher,
    workspace,
    bootstrapSkillSourceDirs,
    options,
  );
  (executor as unknown as Record<string, unknown>)["verifyGolemConnectivity"] = async () => {};
  return executor;
}

class NativeTrackingDriver implements AgentDriver {
  prompts: Array<{ method: "prompt" | "followup"; prompt: string }> = [];
  resetCount = 0;
  private readonly activatedSkills: string[] = [];

  constructor(
    private readonly skillMap: Record<string, string[]>,
    private readonly failureCounts: Record<string, number> = {},
  ) {}

  async setup(_workspace: string, _bootstrapSkillSourceDirs: string[]): Promise<void> {}

  async sendPrompt(prompt: string, _opts: DriverTimeoutOptions): Promise<AgentResult> {
    return this.respond("prompt", prompt);
  }

  async sendFollowup(prompt: string, _opts: DriverTimeoutOptions): Promise<AgentResult> {
    return this.respond("followup", prompt);
  }

  async teardown(): Promise<void> {}

  setWorkingDirectory(_dir: string): void {}

  getActivatedSkills(): string[] | undefined {
    return [...this.activatedSkills];
  }

  getDefaultIdleTimeoutSeconds(): number | undefined {
    return undefined;
  }

  resetActivatedSkills(): void {
    this.resetCount += 1;
    this.activatedSkills.length = 0;
  }

  private async respond(method: "prompt" | "followup", prompt: string): Promise<AgentResult> {
    this.prompts.push({ method, prompt });
    this.activatedSkills.push(...(this.skillMap[prompt] ?? []));

    const remainingFailures = this.failureCounts[prompt] ?? 0;
    if (remainingFailures > 0) {
      this.failureCounts[prompt] = remainingFailures - 1;
      return { success: false, output: `failed: ${prompt}`, durationSeconds: 0, exitCode: 1 };
    }

    return { success: true, output: "ok", durationSeconds: 0, exitCode: 0 };
  }
}

type MockWatcher = SkillWatcher & {
  addSkills(skills: string[]): void;
  getSnapshotCount(): number;
};

function createMockWatcher(): MockWatcher {
  const watcher = new SkillWatcher("/tmp/fake-workspace") as MockWatcher;
  let events: WatcherEvent[] = [];
  let atimeChanges: { skillName: string; path: string }[] = [];
  let snapshotCount = 0;

  watcher.start = async () => {};
  watcher.stop = async () => {};
  watcher.snapshotAtimes = async () => {
    snapshotCount += 1;
    atimeChanges = [];
  };
  watcher.markBaseline = () => events.length;
  watcher.getActivatedEventsSince = (baseline: number) => events.slice(baseline);
  watcher.getSkillsWithChangedAtime = async () => atimeChanges;
  watcher.addSkills = (skills: string[]) => {
    const timestamp = Date.now();
    const nextEvents = skills.map((skillName, index) => ({
      skillName,
      path: `/tmp/${skillName}/SKILL.md`,
      timestamp: timestamp + index,
    }));
    events = events.concat(nextEvents);
    atimeChanges = nextEvents.map(({ skillName, path }) => ({ skillName, path }));
  };
  watcher.getSnapshotCount = () => snapshotCount;

  return watcher;
}

class WatcherTrackingDriver implements AgentDriver {
  prompts: Array<{ method: "prompt" | "followup"; prompt: string }> = [];

  constructor(
    private readonly skillMap: Record<string, string[]>,
    private readonly watcher: MockWatcher,
  ) {}

  async setup(_workspace: string, _bootstrapSkillSourceDirs: string[]): Promise<void> {}

  async sendPrompt(prompt: string, _opts: DriverTimeoutOptions): Promise<AgentResult> {
    return this.respond("prompt", prompt);
  }

  async sendFollowup(prompt: string, _opts: DriverTimeoutOptions): Promise<AgentResult> {
    return this.respond("followup", prompt);
  }

  async teardown(): Promise<void> {}

  setWorkingDirectory(_dir: string): void {}

  getActivatedSkills(): string[] | undefined {
    return undefined;
  }

  getDefaultIdleTimeoutSeconds(): number | undefined {
    return undefined;
  }

  resetActivatedSkills(): void {}

  private async respond(method: "prompt" | "followup", prompt: string): Promise<AgentResult> {
    this.prompts.push({ method, prompt });
    this.watcher.addSkills(this.skillMap[prompt] ?? []);
    return { success: true, output: "ok", durationSeconds: 0, exitCode: 0 };
  }
}

afterEach(async () => {
  await Promise.all(tempDirs.splice(0).map((dir) => fs.rm(dir, { recursive: true, force: true })));
});

describe("assertSkillActivation", () => {
  it("returns undefined when no expected skills", () => {
    const result = assertSkillActivation({}, ["some-skill"]);
    assert.equal(result, undefined);
  });

  it("returns undefined when all expected skills are activated", () => {
    const result = assertSkillActivation({ expectedSkills: ["skill-a", "skill-b"] }, [
      "skill-a",
      "skill-b",
    ]);
    assert.equal(result, undefined);
  });

  it("returns error when expected skill is missing", () => {
    const result = assertSkillActivation({ expectedSkills: ["skill-a", "skill-b"] }, ["skill-a"]);
    assert.ok(result);
    assert.ok(result.includes("SKILL_NOT_ACTIVATED"));
    assert.ok(result.includes("skill-b"));
  });

  it("allows extra skills by default when allowedExtraSkills not set", () => {
    const result = assertSkillActivation({ expectedSkills: ["skill-a"] }, [
      "skill-a",
      "skill-extra",
    ]);
    assert.equal(result, undefined);
  });

  it("allows specified extra skills", () => {
    const result = assertSkillActivation(
      { expectedSkills: ["skill-a"], allowedExtraSkills: ["skill-extra"] },
      ["skill-a", "skill-extra"],
    );
    assert.equal(result, undefined);
  });

  it("returns error for strict match with extras", () => {
    const result = assertSkillActivation({ expectedSkills: ["skill-a"], strictSkillMatch: true }, [
      "skill-a",
      "skill-extra",
    ]);
    assert.ok(result);
    assert.ok(result.includes("SKILL_MISMATCH"));
    assert.ok(result.includes("strict"));
  });

  it("passes strict match when exact", () => {
    const result = assertSkillActivation({ expectedSkills: ["skill-a"], strictSkillMatch: true }, [
      "skill-a",
    ]);
    assert.equal(result, undefined);
  });
});

describe("executeVerification", () => {
  it("does not attempt deploy when expected files are missing", async () => {
    const workspace = await fs.mkdtemp(path.join(os.tmpdir(), "verify-files-"));
    try {
      const executor = Object.create(ScenarioExecutor.prototype) as unknown as ExecutorWithPrivate;
      const calls: string[][] = [];
      const failures: string[] = [];

      executor.workspace = workspace;
      executor.findGolemProjectDir = async () => workspace;
      executor.runLocalCommand = async (_command, args) => {
        calls.push(args);
        return { success: true, output: "ok" };
      };

      await executor.executeVerification(
        "verify",
        { expectedFiles: ["missing.txt"], deploy: true },
        {},
        true,
        (msg) => failures.push(msg),
      );

      assert.deepEqual(calls, []);
      assert.equal(failures.length, 1);
      assert.ok(failures[0].includes("EXPECTED_FILE_MISSING"));
    } finally {
      await fs.rm(workspace, { recursive: true, force: true });
    }
  });

  it("does not attempt deploy after a build failure", async () => {
    const workspace = await fs.mkdtemp(path.join(os.tmpdir(), "verify-build-"));
    try {
      const executor = Object.create(ScenarioExecutor.prototype) as unknown as ExecutorWithPrivate;
      const calls: string[][] = [];
      const failures: string[] = [];

      executor.workspace = workspace;
      executor.findGolemProjectDir = async () => workspace;
      executor.runLocalCommand = async (_command, args) => {
        calls.push(args);
        return { success: false, output: "build failed" };
      };

      await executor.executeVerification("verify", { build: true, deploy: true }, {}, true, (msg) =>
        failures.push(msg),
      );

      assert.deepEqual(calls, [["build"]]);
      assert.equal(failures.length, 1);
      assert.ok(failures[0].includes("BUILD_FAILED"));
    } finally {
      await fs.rm(workspace, { recursive: true, force: true });
    }
  });
});

describe("prompt-session skill tracking", () => {
  it("keeps skill activations cumulative across followups in the same session", async () => {
    const { workspace, bootstrapSkillSourceDirs } = await createTempHarnessDirs();
    const driver = new NativeTrackingDriver({
      "load skill a": ["skill-a"],
      "load skill b": ["skill-b"],
    });
    const watcher = new SkillWatcher(workspace);
    const executor = createExecutor(driver, watcher, workspace, bootstrapSkillSourceDirs);

    const spec: ScenarioSpec = {
      name: "same-session-cumulative",
      steps: [
        {
          id: "step-1",
          tag: "prompt",
          prompt: "load skill a",
          expectedSkills: ["skill-a"],
        },
        {
          id: "step-2",
          tag: "prompt",
          prompt: "load skill b",
          expectedSkills: ["skill-b"],
        },
      ],
    };

    const result = await executor.execute(spec);
    assert.equal(result.status, "pass");
    assert.deepEqual(
      driver.prompts.map(({ method }) => method),
      ["prompt", "followup"],
    );
    assert.equal(driver.resetCount, 1);
    assert.deepEqual(result.stepResults[1].activatedSkills, ["skill-a", "skill-b"]);
  });

  it("resets skill tracking when continueSession is false", async () => {
    const { workspace, bootstrapSkillSourceDirs } = await createTempHarnessDirs();
    const driver = new NativeTrackingDriver({
      "load skill a": ["skill-a"],
      "load skill b": ["skill-b"],
    });
    const watcher = new SkillWatcher(workspace);
    const executor = createExecutor(driver, watcher, workspace, bootstrapSkillSourceDirs);

    const spec: ScenarioSpec = {
      name: "fresh-session-reset",
      steps: [
        {
          id: "step-1",
          tag: "prompt",
          prompt: "load skill a",
          expectedSkills: ["skill-a"],
        },
        {
          id: "step-2",
          tag: "prompt",
          prompt: "load skill b",
          continueSession: false,
          expectedSkills: ["skill-b"],
        },
      ],
    };

    const result = await executor.execute(spec);
    assert.equal(result.status, "pass");
    assert.deepEqual(
      driver.prompts.map(({ method }) => method),
      ["prompt", "prompt"],
    );
    assert.equal(driver.resetCount, 2);
    assert.deepEqual(result.stepResults[1].activatedSkills, ["skill-b"]);
  });

  it("starts watcher-based tracking on an untracked session-opening prompt", async () => {
    const { workspace, bootstrapSkillSourceDirs } = await createTempHarnessDirs();
    const watcher = createMockWatcher();
    const driver = new WatcherTrackingDriver(
      {
        "load skill a": ["skill-a"],
        "load skill b": ["skill-b"],
      },
      watcher,
    );
    const executor = createExecutor(driver, watcher, workspace, bootstrapSkillSourceDirs);

    const spec: ScenarioSpec = {
      name: "watcher-session-baseline",
      steps: [
        {
          id: "step-1",
          tag: "prompt",
          prompt: "load skill a",
        },
        {
          id: "step-2",
          tag: "prompt",
          prompt: "load skill b",
          expectedSkills: ["skill-a", "skill-b"],
        },
      ],
    };

    const result = await executor.execute(spec);
    assert.equal(result.status, "pass");
    assert.deepEqual(
      driver.prompts.map(({ method }) => method),
      ["prompt", "followup"],
    );
    assert.equal(watcher.getSnapshotCount(), 1);
    assert.deepEqual(result.stepResults[1].activatedSkills, ["skill-a", "skill-b"]);
  });

  it("retries a failed initial prompt as a fresh prompt", async () => {
    const { workspace, bootstrapSkillSourceDirs } = await createTempHarnessDirs();
    const driver = new NativeTrackingDriver({ "load skill a": ["skill-a"] }, { "load skill a": 1 });
    const watcher = new SkillWatcher(workspace);
    const executor = createExecutor(driver, watcher, workspace, bootstrapSkillSourceDirs);

    const spec: ScenarioSpec = {
      name: "retry-fresh-initial-prompt",
      steps: [
        {
          id: "step-1",
          tag: "prompt",
          prompt: "load skill a",
          expectedSkills: ["skill-a"],
          retry: { attempts: 2, delay: 0 },
        },
      ],
    };

    const result = await executor.execute(spec);
    assert.equal(result.status, "pass");
    assert.deepEqual(
      driver.prompts.map(({ method }) => method),
      ["prompt", "prompt"],
    );
    assert.equal(driver.resetCount, 2);
  });
});

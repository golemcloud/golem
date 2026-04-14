import { describe, it } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { renderGitHubStepSummary } from "../src/summary.js";

/**
 * Test that the GitHub summary writing logic works by simulating
 * the exact code path from run.ts.
 */
describe("GitHub Actions summary", () => {
  it("writes markdown table to GITHUB_STEP_SUMMARY file", async () => {
    const tmpDir = await fs.mkdtemp(path.join(os.tmpdir(), "gh-summary-"));
    const summaryFile = path.join(tmpDir, "summary.md");

    const scenarioReports = [
      {
        scenario: "test-pass",
        matrix: { agent: "claude-code", language: "ts" },
        status: "pass" as const,
        durationSeconds: 12.5,
      },
      {
        scenario: "test-fail",
        matrix: { agent: "claude-code", language: "ts" },
        status: "fail" as const,
        durationSeconds: 5.3,
      },
    ];

    const totalScenarios = scenarioReports.length;
    const passed = scenarioReports.filter((r) => r.status === "pass").length;
    const failed = scenarioReports.filter((r) => r.status === "fail").length;
    const totalDuration = scenarioReports.reduce((sum, r) => sum + r.durationSeconds, 0);
    const worstFailures = [
      {
        scenario: "test-fail",
        matrix: { agent: "claude-code", language: "ts" },
        error: "SHELL_FAILED: exit code 1",
        guidance: "Inspect stderr output",
      },
    ];

    await fs.appendFile(
      summaryFile,
      renderGitHubStepSummary({
        scenarioReports,
        totalScenarios,
        passed,
        failed,
        totalDuration,
        worstFailures,
      }),
    );

    const content = await fs.readFile(summaryFile, "utf8");
    assert.ok(content.includes("## Skill Test Results"));
    assert.ok(content.includes("| Scenario | Agent | Language | Status | Duration |"));
    assert.ok(content.includes("| test-pass |"));
    assert.ok(content.includes("| test-fail |"));
    assert.ok(content.includes("| claude-code | ts |"));
    assert.ok(content.includes("\u2705 pass"));
    assert.ok(content.includes("\u274c fail"));
    assert.ok(content.includes("**Total:** 2"));
    assert.ok(content.includes("**Passed:** 1"));
    assert.ok(content.includes("**Failed:** 1"));
    assert.ok(content.includes("### Failures"));
    assert.ok(content.includes("test-fail [claude-code x ts]"));
    assert.ok(content.includes("SHELL_FAILED"));
    assert.ok(content.includes("Inspect stderr output"));
    assert.ok(content.includes("17.8s")); // 12.5 + 5.3

    await fs.appendFile(
      summaryFile,
      renderGitHubStepSummary({
        scenarioReports,
        totalScenarios,
        passed,
        failed,
        totalDuration,
        worstFailures,
      }),
    );
    const content2 = await fs.readFile(summaryFile, "utf8");
    const count = (content2.match(/## Skill Test Results/g) ?? []).length;
    assert.equal(count, 2);

    await fs.rm(tmpDir, { recursive: true });
  });
});

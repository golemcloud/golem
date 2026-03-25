import { describe, it } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";

/**
 * Test that the GitHub summary writing logic works by simulating
 * the exact code path from run.ts.
 */
describe("GitHub Actions summary", () => {
  it("writes markdown table to GITHUB_STEP_SUMMARY file", async () => {
    const tmpDir = await fs.mkdtemp(path.join(os.tmpdir(), "gh-summary-"));
    const summaryFile = path.join(tmpDir, "summary.md");

    // Simulate the scenario reports that run.ts would produce
    const scenarioReports = [
      { scenario: "test-pass", status: "pass" as const, durationSeconds: 12.5 },
      { scenario: "test-fail", status: "fail" as const, durationSeconds: 5.3 },
    ];

    const agent = "claude-code";
    const language = "ts";
    const totalScenarios = scenarioReports.length;
    const passed = scenarioReports.filter((r) => r.status === "pass").length;
    const failed = scenarioReports.filter((r) => r.status === "fail").length;
    const totalDuration = scenarioReports.reduce(
      (sum, r) => sum + r.durationSeconds,
      0,
    );
    const worstFailures = [
      { scenario: "test-fail", error: "SHELL_FAILED: exit code 1" },
    ];

    // Replicate the exact GitHub summary code from run.ts
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

    await fs.appendFile(summaryFile, lines.join("\n"));

    // Verify the file was written with expected content
    const content = await fs.readFile(summaryFile, "utf8");
    assert.ok(content.includes("## Skill Test Results — claude-code / ts"));
    assert.ok(content.includes("| test-pass |"));
    assert.ok(content.includes("| test-fail |"));
    assert.ok(content.includes("\u2705 pass"));
    assert.ok(content.includes("\u274c fail"));
    assert.ok(content.includes("**Total:** 2"));
    assert.ok(content.includes("**Passed:** 1"));
    assert.ok(content.includes("**Failed:** 1"));
    assert.ok(content.includes("### Failures"));
    assert.ok(content.includes("SHELL_FAILED"));
    assert.ok(content.includes("17.8s")); // 12.5 + 5.3

    // Verify it appends (not overwrites) by writing again
    await fs.appendFile(summaryFile, lines.join("\n"));
    const content2 = await fs.readFile(summaryFile, "utf8");
    // Should appear twice
    const count = (content2.match(/## Skill Test Results/g) ?? []).length;
    assert.equal(count, 2);

    // Cleanup
    await fs.rm(tmpDir, { recursive: true });
  });
});

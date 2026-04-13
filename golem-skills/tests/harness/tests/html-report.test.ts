import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { generateHtmlReport, escapeHtml } from "../src/html-report.js";
import type { Summary, MergedSummary, HtmlScenarioReport } from "../src/html-report.js";
import type { StepSpec } from "../src/executor.js";

function makePromptStep(id: string, prompt: string): StepSpec {
  return { tag: "prompt", id, prompt };
}

describe("escapeHtml", () => {
  it("escapes HTML special characters", () => {
    assert.equal(
      escapeHtml('<script>alert("xss")</script>'),
      "&lt;script&gt;alert(&quot;xss&quot;)&lt;/script&gt;",
    );
  });

  it("escapes ampersands", () => {
    assert.equal(escapeHtml("a & b"), "a &amp; b");
  });

  it("escapes single quotes", () => {
    assert.equal(escapeHtml("it's"), "it&#39;s");
  });

  it("returns empty string for empty input", () => {
    assert.equal(escapeHtml(""), "");
  });

  it("leaves safe strings unchanged", () => {
    assert.equal(escapeHtml("hello world"), "hello world");
  });
});

describe("generateHtmlReport", () => {
  const baseSummary: Summary = {
    agent: "claude-code",
    language: "ts",
    os: "darwin",
    timestamp: "2026-01-01T00:00:00.000Z",
    total: 2,
    passed: 1,
    failed: 1,
    skipped: 0,
    durationSeconds: 42.5,
    worstFailures: [{ scenario: "failing-scenario", error: "BUILD_FAILED" }],
    scenarios: [
      { name: "passing-scenario", status: "pass", durationSeconds: 10 },
      { name: "failing-scenario", status: "fail", durationSeconds: 32.5 },
    ],
  };

  const sampleReports: HtmlScenarioReport[] = [
    {
      scenario: "passing-scenario",
      matrix: { agent: "claude-code", language: "ts" },
      run_id: "123",
      status: "pass",
      durationSeconds: 10,
      results: [
        {
          step: makePromptStep("step-1", "Do something"),
          success: true,
          durationSeconds: 10,
          expectedSkills: [],
          activatedSkills: [],
        },
      ],
      artifactPaths: [],
    },
    {
      scenario: "failing-scenario",
      matrix: { agent: "claude-code", language: "ts" },
      run_id: "456",
      status: "fail",
      durationSeconds: 32.5,
      results: [
        {
          step: makePromptStep("step-1", "Build it"),
          success: false,
          durationSeconds: 32.5,
          expectedSkills: [],
          activatedSkills: [],
          error: "BUILD_FAILED: exit code 1",
        },
      ],
      artifactPaths: [],
    },
  ];

  it("generates valid HTML document", () => {
    const html = generateHtmlReport(baseSummary, sampleReports);
    assert.ok(html.includes("<!DOCTYPE html>"));
    assert.ok(html.includes('<html lang="en">'));
    assert.ok(html.includes("</html>"));
  });

  it("includes overview section with agent/language/os", () => {
    const html = generateHtmlReport(baseSummary, sampleReports);
    assert.ok(html.includes("claude-code"));
    assert.ok(html.includes("darwin"));
    assert.ok(html.includes("2026-01-01"));
  });

  it("includes scenario details", () => {
    const html = generateHtmlReport(baseSummary, sampleReports);
    assert.ok(html.includes("passing-scenario"));
    assert.ok(html.includes("failing-scenario"));
  });

  it("includes failure summary", () => {
    const html = generateHtmlReport(baseSummary, sampleReports);
    assert.ok(html.includes("Failures"));
    assert.ok(html.includes("BUILD_FAILED"));
  });

  it("escapes user strings in scenario names", () => {
    const reports: HtmlScenarioReport[] = [
      {
        scenario: '<script>alert("xss")</script>',
        matrix: { agent: "test", language: "ts" },
        run_id: "789",
        status: "pass",
        durationSeconds: 1,
        results: [
          {
            step: makePromptStep("step-1", "test"),
            success: true,
            durationSeconds: 1,
            expectedSkills: [],
            activatedSkills: [],
          },
        ],
        artifactPaths: [],
      },
    ];

    const html = generateHtmlReport(baseSummary, reports);
    assert.ok(!html.includes("<script>alert"));
    assert.ok(html.includes("&lt;script&gt;"));
  });

  it("includes step attempt details when present", () => {
    const reports: HtmlScenarioReport[] = [
      {
        scenario: "retry-scenario",
        matrix: { agent: "claude-code", language: "ts" },
        run_id: "100",
        status: "pass",
        durationSeconds: 20,
        results: [
          {
            step: makePromptStep("flaky", "flaky step"),
            success: true,
            durationSeconds: 20,
            expectedSkills: [],
            activatedSkills: [],
            attempts: [
              {
                attemptNumber: 1,
                success: false,
                durationSeconds: 8,
                error: "timeout",
                activatedSkills: [],
              },
              {
                attemptNumber: 2,
                success: true,
                durationSeconds: 12,
                activatedSkills: [],
              },
            ],
          },
        ],
        artifactPaths: [],
      },
    ];

    const html = generateHtmlReport(baseSummary, reports);
    assert.ok(html.includes("Attempts:"));
    assert.ok(html.includes("#1 fail"));
    assert.ok(html.includes("#2 pass"));
  });

  it("includes classification details when present", () => {
    const reports: HtmlScenarioReport[] = [
      {
        scenario: "classified-failure",
        matrix: { agent: "claude-code", language: "ts" },
        run_id: "200",
        status: "fail",
        durationSeconds: 5,
        results: [
          {
            step: makePromptStep("failing", "build it"),
            success: false,
            durationSeconds: 5,
            expectedSkills: [],
            activatedSkills: [],
            error: "BUILD_FAILED: exit code 1",
            classification: {
              code: "BUILD_FAILED",
              category: "build",
              guidance: "Check that golem.yaml exists",
            },
          },
        ],
        artifactPaths: [],
      },
    ];

    const html = generateHtmlReport(baseSummary, reports);
    assert.ok(html.includes("build"));
    assert.ok(html.includes("Check that golem.yaml exists"));
  });

  it("handles empty scenario reports", () => {
    const html = generateHtmlReport(baseSummary, []);
    assert.ok(html.includes("<!DOCTYPE html>"));
    assert.ok(html.includes("Test Report"));
  });

  it("generates merged report with matrix table", () => {
    const merged: MergedSummary = {
      overallTotal: 4,
      overallPassed: 3,
      overallFailed: 1,
      matrix: {
        agents: ["claude-code", "opencode"],
        languages: ["ts"],
        os: ["linux"],
      },
      heatMap: [
        {
          agent: "claude-code",
          language: "ts",
          os: "linux",
          total: 2,
          passed: 2,
          failed: 0,
        },
        {
          agent: "opencode",
          language: "ts",
          os: "linux",
          total: 2,
          passed: 1,
          failed: 1,
        },
      ],
      summaries: [],
    };

    const html = generateHtmlReport(merged, []);
    assert.ok(html.includes("Merged Test Report"));
    assert.ok(html.includes("Matrix Results"));
    assert.ok(html.includes("claude-code"));
    assert.ok(html.includes("opencode"));
  });
});

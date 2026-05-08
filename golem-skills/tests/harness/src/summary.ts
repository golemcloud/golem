export interface ScenarioMatrix {
  agent: string;
  language: string;
  model?: string;
}

export interface MatrixScenarioLabel {
  scenario: string;
  matrix: ScenarioMatrix;
}

export interface GitHubSummaryScenario extends MatrixScenarioLabel {
  status: "pass" | "fail";
  durationSeconds: number;
  usage?: { inputTokens?: number; outputTokens?: number; costUsd?: number };
}

export interface GitHubSummaryFailure extends MatrixScenarioLabel {
  error: string;
  guidance?: string;
}

export function formatScenarioMatrixLabel({ scenario, matrix }: MatrixScenarioLabel): string {
  const agentLabel = matrix.model ? `${matrix.agent}/${matrix.model}` : matrix.agent;
  return `${scenario} [${agentLabel} x ${matrix.language}]`;
}

export function renderGitHubStepSummary(options: {
  scenarioReports: GitHubSummaryScenario[];
  totalScenarios: number;
  passed: number;
  failed: number;
  totalDuration: number;
  worstFailures: GitHubSummaryFailure[];
}): string {
  const { scenarioReports, totalScenarios, passed, failed, totalDuration, worstFailures } = options;

  const lines: string[] = [];
  lines.push("## Skill Test Results");
  lines.push("");
  const anyUsage = scenarioReports.some((r) => r.usage);
  if (anyUsage) {
    lines.push("| Scenario | Agent | Language | Status | Duration | Tokens | Cost |");
    lines.push("|----------|-------|----------|--------|----------|--------|------|");
  } else {
    lines.push("| Scenario | Agent | Language | Status | Duration |");
    lines.push("|----------|-------|----------|--------|----------|");
  }
  for (const report of scenarioReports) {
    const icon = report.status === "pass" ? "\u2705" : "\u274c";
    const baseCols = `| ${report.scenario} | ${report.matrix.agent} | ${report.matrix.language} | ${icon} ${report.status} | ${report.durationSeconds.toFixed(1)}s`;
    if (anyUsage) {
      const u = report.usage;
      const tokens =
        u?.inputTokens || u?.outputTokens
          ? `${(u?.inputTokens ?? 0).toLocaleString()}/${(u?.outputTokens ?? 0).toLocaleString()}`
          : "-";
      const cost = u?.costUsd ? `$${u.costUsd.toFixed(4)}` : "-";
      lines.push(`${baseCols} | ${tokens} | ${cost} |`);
    } else {
      lines.push(`${baseCols} |`);
    }
  }
  lines.push("");
  lines.push(
    `**Total:** ${totalScenarios} | **Passed:** ${passed} | **Failed:** ${failed} | **Duration:** ${totalDuration.toFixed(1)}s`,
  );

  if (worstFailures.length > 0) {
    lines.push("");
    lines.push("### Failures");
    for (const failure of worstFailures) {
      const truncatedError =
        failure.error.length > 200 ? `${failure.error.slice(0, 197)}...` : failure.error;
      lines.push(`- **${formatScenarioMatrixLabel(failure)}**: ${truncatedError}`);
      if (failure.guidance) {
        lines.push(`  - _${failure.guidance}_`);
      }
    }
  }

  lines.push("");
  return lines.join("\n");
}

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
  lines.push("| Scenario | Agent | Language | Status | Duration |");
  lines.push("|----------|-------|----------|--------|----------|");
  for (const report of scenarioReports) {
    const icon = report.status === "pass" ? "\u2705" : "\u274c";
    lines.push(
      `| ${report.scenario} | ${report.matrix.agent} | ${report.matrix.language} | ${icon} ${report.status} | ${report.durationSeconds.toFixed(1)}s |`,
    );
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

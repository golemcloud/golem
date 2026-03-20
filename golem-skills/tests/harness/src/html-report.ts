import type { StepResult } from './executor.js';

interface ScenarioReport {
  scenario: string;
  matrix: { agent: string; language: string };
  run_id: string;
  status: 'pass' | 'fail';
  durationSeconds: number;
  results: StepResult[];
  artifactPaths: string[];
}

interface Summary {
  agent: string;
  language: string;
  os: string;
  timestamp: string;
  total: number;
  passed: number;
  failed: number;
  skipped: number;
  durationSeconds: number;
  worstFailures: Array<{ scenario: string; error: string }>;
  scenarios: Array<{ name: string; status: string; durationSeconds: number }>;
}

interface MergedSummary {
  overallTotal: number;
  overallPassed: number;
  overallFailed: number;
  matrix: { agents: string[]; languages: string[]; os: string[] };
  heatMap: Array<{ agent: string; language: string; os: string; total: number; passed: number; failed: number }>;
  summaries: Summary[];
}

function escapeHtml(str: string): string {
  return str
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

function isMergedSummary(summary: Summary | MergedSummary): summary is MergedSummary {
  return 'heatMap' in summary;
}

function generateOverviewSection(summary: Summary | MergedSummary): string {
  if (isMergedSummary(summary)) {
    return `
    <div class="overview">
      <h2>Overview</h2>
      <div class="stats">
        <div class="stat"><span class="label">Total</span><span class="value">${summary.overallTotal}</span></div>
        <div class="stat pass"><span class="label">Passed</span><span class="value">${summary.overallPassed}</span></div>
        <div class="stat fail"><span class="label">Failed</span><span class="value">${summary.overallFailed}</span></div>
      </div>
    </div>`;
  }

  return `
    <div class="overview">
      <h2>Overview</h2>
      <div class="meta">
        <span>Agent: <strong>${escapeHtml(summary.agent)}</strong></span>
        <span>Language: <strong>${escapeHtml(summary.language)}</strong></span>
        <span>OS: <strong>${escapeHtml(summary.os)}</strong></span>
        <span>Timestamp: <strong>${escapeHtml(summary.timestamp)}</strong></span>
      </div>
      <div class="stats">
        <div class="stat"><span class="label">Total</span><span class="value">${summary.total}</span></div>
        <div class="stat pass"><span class="label">Passed</span><span class="value">${summary.passed}</span></div>
        <div class="stat fail"><span class="label">Failed</span><span class="value">${summary.failed}</span></div>
        <div class="stat skip"><span class="label">Skipped</span><span class="value">${summary.skipped}</span></div>
        <div class="stat"><span class="label">Duration</span><span class="value">${summary.durationSeconds.toFixed(1)}s</span></div>
      </div>
    </div>`;
}

function generateMatrixTable(summary: MergedSummary): string {
  if (summary.heatMap.length === 0) return '';

  const rows = summary.heatMap.map(entry => {
    const statusClass = entry.failed > 0 ? 'fail' : 'pass';
    return `<tr class="${statusClass}">
      <td>${escapeHtml(entry.agent)}</td>
      <td>${escapeHtml(entry.language)}</td>
      <td>${escapeHtml(entry.os)}</td>
      <td>${entry.total}</td>
      <td>${entry.passed}</td>
      <td>${entry.failed}</td>
    </tr>`;
  }).join('\n');

  return `
    <div class="matrix-table">
      <h2>Matrix Results</h2>
      <table>
        <thead><tr><th>Agent</th><th>Language</th><th>OS</th><th>Total</th><th>Passed</th><th>Failed</th></tr></thead>
        <tbody>${rows}</tbody>
      </table>
    </div>`;
}

function generateScenarioDetails(reports: ScenarioReport[]): string {
  if (reports.length === 0) return '';

  const sections = reports.map(report => {
    const statusIcon = report.status === 'pass' ? '&#10003;' : '&#10007;';
    const statusClass = report.status === 'pass' ? 'pass' : 'fail';

    const steps = report.results.map((r, i) => {
      const stepName = escapeHtml(r.step.id ?? r.step.prompt ?? `step-${i + 1}`);
      const sClass = r.success ? 'pass' : 'fail';
      const errorBlock = r.error
        ? `<pre class="error">${escapeHtml(r.error)}</pre>`
        : '';
      return `<div class="step ${sClass}">
        <span class="step-name">${stepName}</span>
        <span class="step-duration">${r.durationSeconds.toFixed(1)}s</span>
        ${errorBlock}
      </div>`;
    }).join('\n');

    return `
    <details>
      <summary class="${statusClass}">
        <span>${statusIcon}</span>
        <span>${escapeHtml(report.scenario)}</span>
        <span class="duration">${report.durationSeconds.toFixed(1)}s</span>
      </summary>
      <div class="scenario-body">${steps}</div>
    </details>`;
  }).join('\n');

  return `
    <div class="scenarios">
      <h2>Scenarios</h2>
      ${sections}
    </div>`;
}

function generateFailureSummary(reports: ScenarioReport[]): string {
  const failures = reports.filter(r => r.status === 'fail');
  if (failures.length === 0) return '';

  const items = failures.map(r => {
    const failedStep = r.results.find(s => !s.success);
    const error = failedStep?.error ?? 'unknown';
    return `<li><strong>${escapeHtml(r.scenario)}</strong>: ${escapeHtml(error)}</li>`;
  }).join('\n');

  return `
    <div class="failure-summary">
      <h2>Failures</h2>
      <ul>${items}</ul>
    </div>`;
}

const CSS = `
  * { box-sizing: border-box; margin: 0; padding: 0; }
  body { font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; max-width: 960px; margin: 0 auto; padding: 24px; background: #f8f9fa; color: #1a1a2e; }
  h1 { margin-bottom: 24px; font-size: 1.5rem; }
  h2 { margin-bottom: 12px; font-size: 1.2rem; border-bottom: 1px solid #dee2e6; padding-bottom: 6px; }
  .overview, .matrix-table, .scenarios, .failure-summary { background: #fff; border-radius: 8px; padding: 16px; margin-bottom: 16px; border: 1px solid #dee2e6; }
  .meta { display: flex; gap: 16px; flex-wrap: wrap; margin-bottom: 12px; font-size: 0.9rem; }
  .stats { display: flex; gap: 16px; flex-wrap: wrap; }
  .stat { display: flex; flex-direction: column; align-items: center; padding: 8px 16px; border-radius: 6px; background: #e9ecef; }
  .stat .label { font-size: 0.75rem; text-transform: uppercase; color: #6c757d; }
  .stat .value { font-size: 1.25rem; font-weight: bold; }
  .stat.pass .value { color: #198754; }
  .stat.fail .value { color: #dc3545; }
  .stat.skip .value { color: #fd7e14; }
  table { width: 100%; border-collapse: collapse; }
  th, td { padding: 8px 12px; text-align: left; border-bottom: 1px solid #dee2e6; }
  th { background: #e9ecef; font-size: 0.85rem; text-transform: uppercase; }
  tr.pass td:first-child { border-left: 3px solid #198754; }
  tr.fail td:first-child { border-left: 3px solid #dc3545; }
  details { margin-bottom: 8px; }
  summary { cursor: pointer; padding: 8px 12px; border-radius: 6px; display: flex; gap: 8px; align-items: center; }
  summary.pass { background: #d1e7dd; }
  summary.fail { background: #f8d7da; }
  summary .duration { margin-left: auto; font-size: 0.85rem; color: #6c757d; }
  .scenario-body { padding: 8px 16px; }
  .step { padding: 6px 8px; margin: 4px 0; border-radius: 4px; display: flex; flex-wrap: wrap; gap: 8px; align-items: center; }
  .step.pass { background: #d1e7dd; }
  .step.fail { background: #f8d7da; }
  .step-name { font-weight: 500; }
  .step-duration { font-size: 0.85rem; color: #6c757d; margin-left: auto; }
  .attempts { width: 100%; font-size: 0.8rem; color: #6c757d; }
  pre.error { width: 100%; background: #2b2d42; color: #edf2f4; padding: 8px; border-radius: 4px; font-size: 0.8rem; overflow-x: auto; white-space: pre-wrap; }
  .failure-summary ul { list-style: none; padding-left: 0; }
  .failure-summary li { padding: 6px 0; border-bottom: 1px solid #dee2e6; }
  .failure-summary li:last-child { border-bottom: none; }
`;

export function generateHtmlReport(summary: Summary | MergedSummary, scenarioReports: ScenarioReport[]): string {
  const title = isMergedSummary(summary) ? 'Merged Test Report' : 'Test Report';
  const matrixSection = isMergedSummary(summary) ? generateMatrixTable(summary) : '';

  return `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>${escapeHtml(title)}</title>
  <style>${CSS}</style>
</head>
<body>
  <h1>${escapeHtml(title)}</h1>
  ${generateOverviewSection(summary)}
  ${matrixSection}
  ${generateScenarioDetails(scenarioReports)}
  ${generateFailureSummary(scenarioReports)}
</body>
</html>`;
}

export { escapeHtml };
export type { Summary, MergedSummary, ScenarioReport as HtmlScenarioReport };

import type { StepResult } from "./executor.js";

interface ScenarioReport {
  scenario: string;
  matrix: { agent: string; language: string; model?: string };
  run_id: string;
  status: "pass" | "fail";
  durationSeconds: number;
  results: StepResult[];
  artifactPaths: string[];
}

export interface Summary {
  agent: string;
  model?: string;
  language: string;
  os: string;
  timestamp: string;
  total: number;
  passed: number;
  failed: number;
  skipped: number;
  durationSeconds: number;
  worstFailures: Array<{ scenario: string; agent?: string; language?: string; error: string }>;
  scenarios: Array<{ name: string; status: string; durationSeconds: number }>;
}

export interface MergedSummary {
  overallTotal: number;
  overallPassed: number;
  overallFailed: number;
  matrix: { agents: string[]; languages: string[]; os: string[] };
  heatMap: Array<{
    agent: string;
    model?: string;
    language: string;
    os: string;
    total: number;
    passed: number;
    failed: number;
  }>;
  summaries: Summary[];
}

export function escapeHtml(str: string): string {
  return str
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function isMergedSummary(summary: Summary | MergedSummary): summary is MergedSummary {
  return "heatMap" in summary;
}

function driverLabel(agent: string, model?: string): string {
  return model ? `${agent}/${model}` : agent;
}

function generateOverviewSection(summary: Summary | MergedSummary): string {
  if (isMergedSummary(summary)) {
    const agentLabels = [...new Set(summary.heatMap.map((e) => driverLabel(e.agent, e.model)))];
    const agentBadges = agentLabels
      .map((l) => `<span class="agent-label">${escapeHtml(l)}</span>`)
      .join(" ");
    return `
    <div class="overview">
      <h2>Overview</h2>
      <div class="stats">
        <div class="stat"><span class="label">Total</span><span class="value">${summary.overallTotal}</span></div>
        <div class="stat pass"><span class="label">Passed</span><span class="value">${summary.overallPassed}</span></div>
        <div class="stat fail"><span class="label">Failed</span><span class="value">${summary.overallFailed}</span></div>
      </div>
      <div class="agent-labels"><span class="label">Agents:</span> ${agentBadges}</div>
    </div>`;
  }

  return `
    <div class="overview">
      <h2>Overview</h2>
      <div class="meta">
        <span>Agent: <strong>${escapeHtml(driverLabel(summary.agent, summary.model))}</strong></span>
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
  if (summary.heatMap.length === 0) return "";

  const rows = summary.heatMap
    .map((entry) => {
      const statusClass = entry.failed > 0 ? "fail" : "pass";
      const agentCell = escapeHtml(driverLabel(entry.agent, entry.model));
      return `<tr class="${statusClass}">
      <td>${agentCell}</td>
      <td>${escapeHtml(entry.language)}</td>
      <td>${escapeHtml(entry.os)}</td>
      <td>${entry.total}</td>
      <td>${entry.passed}</td>
      <td>${entry.failed}</td>
    </tr>`;
    })
    .join("\n");

  return `
    <div class="matrix-table">
      <h2>Matrix Results</h2>
      <table>
        <thead><tr><th>Agent</th><th>Language</th><th>OS</th><th>Total</th><th>Passed</th><th>Failed</th></tr></thead>
        <tbody>${rows}</tbody>
      </table>
    </div>`;
}

function generateScenarioMatrix(reports: ScenarioReport[]): string {
  if (reports.length === 0) return "";

  const drivers = [
    ...new Set(reports.map((r) => driverLabel(r.matrix.agent, r.matrix.model))),
  ].sort();
  const scenarios = [...new Set(reports.map((r) => r.scenario))].sort();
  const languages = [...new Set(reports.map((r) => r.matrix.language))].sort();

  const driverHeaders = drivers.map((d) => `<th class="driver-col">${escapeHtml(d)}</th>`).join("");

  const rows = scenarios
    .map((scenario) => {
      const cells = drivers
        .map((driver) => {
          return `<td class="matrix-cell" data-scenario="${escapeHtml(scenario)}" data-driver="${escapeHtml(driver)}"></td>`;
        })
        .join("");
      return `<tr><td class="scenario-name">${escapeHtml(scenario)}</td>${cells}</tr>`;
    })
    .join("\n");

  const languageOptions = languages
    .map((l) => `<option value="${escapeHtml(l)}">${escapeHtml(l)}</option>`)
    .join("");

  return `
    <div class="scenario-matrix">
      <h2>Scenarios</h2>
      <div class="matrix-controls">
        <label for="language-filter">Language:</label>
        <select id="language-filter">
          <option value="__all__">All languages</option>
          ${languageOptions}
        </select>
      </div>
      <div class="matrix-table-wrap">
        <table class="interactive-matrix" id="scenario-matrix-table">
          <thead><tr><th class="scenario-col">Scenario</th>${driverHeaders}</tr></thead>
          <tbody>${rows}</tbody>
        </table>
      </div>
      <div id="detail-panel"></div>
    </div>`;
}

function generateFailureSummary(
  summary: Summary | MergedSummary,
  reports: ScenarioReport[],
): string {
  const failures = reports
    .filter((report) => report.status === "fail")
    .map((report) => {
      const failedStep = report.results.find((step) => !step.success);
      return {
        scenario: report.scenario,
        driver: driverLabel(report.matrix.agent, report.matrix.model),
        language: report.matrix.language,
        stepName: failedStep?.step.id ?? "unknown step",
        error: failedStep?.error ?? "unknown",
        classification: failedStep?.classification,
      };
    });

  if (failures.length === 0) {
    const summaryFailures = isMergedSummary(summary)
      ? summary.summaries.flatMap((entry) => entry.worstFailures)
      : summary.worstFailures;
    failures.push(
      ...summaryFailures.map((failure) => ({
        scenario: failure.scenario,
        driver: failure.agent ?? "unknown",
        language: failure.language ?? "unknown",
        stepName: "unknown step",
        error: failure.error,
        classification: undefined,
      })),
    );
  }

  if (failures.length === 0) return "";

  const items = failures
    .map((f) => {
      const classificationBadge = f.classification
        ? `<span class="badge ${f.classification.category}">${escapeHtml(f.classification.category)}</span>`
        : "";
      return `<div class="failure-item">
        <div class="failure-header">
          <strong class="failure-scenario">${escapeHtml(f.scenario)}</strong>
          ${classificationBadge}
        </div>
        <div class="failure-meta">
          <span>Driver: <strong>${escapeHtml(f.driver)}</strong></span>
          <span>Language: <strong>${escapeHtml(f.language)}</strong></span>
          <span>Step: <strong>${escapeHtml(f.stepName)}</strong></span>
        </div>
        <pre class="error">${escapeHtml(f.error)}</pre>
      </div>`;
    })
    .join("\n");

  return `
    <div class="failure-summary">
      <h2>Failures</h2>
      ${items}
    </div>`;
}

const CSS = `
  * { box-sizing: border-box; margin: 0; padding: 0; }
  body { font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; max-width: 1100px; margin: 0 auto; padding: 24px; background: #f8f9fa; color: #1a1a2e; }
  h1 { margin-bottom: 24px; font-size: 1.5rem; }
  h2 { margin-bottom: 12px; font-size: 1.2rem; border-bottom: 1px solid #dee2e6; padding-bottom: 6px; }
  .overview, .matrix-table, .scenario-matrix, .failure-summary { background: #fff; border-radius: 8px; padding: 16px; margin-bottom: 16px; border: 1px solid #dee2e6; }
  .meta { display: flex; gap: 16px; flex-wrap: wrap; margin-bottom: 12px; font-size: 0.9rem; }
  .stats { display: flex; gap: 16px; flex-wrap: wrap; }
  .stat { display: flex; flex-direction: column; align-items: center; padding: 8px 16px; border-radius: 6px; background: #e9ecef; }
  .stat .label { font-size: 0.75rem; text-transform: uppercase; color: #6c757d; }
  .stat .value { font-size: 1.25rem; font-weight: bold; }
  .stat.pass .value { color: #198754; }
  .stat.fail .value { color: #dc3545; }
  .stat.skip .value { color: #fd7e14; }
  .agent-labels { margin-top: 12px; font-size: 0.9rem; }
  .agent-labels .label { color: #6c757d; text-transform: uppercase; font-size: 0.75rem; margin-right: 4px; }
  .agent-label { display: inline-block; padding: 2px 8px; border-radius: 4px; background: #e9ecef; font-weight: 500; margin: 2px 4px; font-size: 0.85rem; }
  table { width: 100%; border-collapse: collapse; }
  th, td { padding: 8px 12px; text-align: left; border-bottom: 1px solid #dee2e6; }
  th { background: #e9ecef; font-size: 0.85rem; text-transform: uppercase; }
  tr.pass td:first-child { border-left: 3px solid #198754; }
  tr.fail td:first-child { border-left: 3px solid #dc3545; }

  /* Scenario matrix */
  .matrix-controls { margin-bottom: 12px; display: flex; align-items: center; gap: 8px; }
  .matrix-controls label { font-size: 0.85rem; font-weight: 500; }
  .matrix-controls select { padding: 4px 8px; border-radius: 4px; border: 1px solid #ced4da; font-size: 0.85rem; }
  .matrix-table-wrap { overflow-x: auto; }
  .interactive-matrix th.scenario-col { min-width: 200px; }
  .interactive-matrix th.driver-col { text-align: center; min-width: 90px; font-size: 0.75rem; }
  .matrix-cell { text-align: center; cursor: pointer; font-weight: bold; font-size: 1rem; transition: opacity 0.15s; user-select: none; }
  .matrix-cell:hover { opacity: 0.75; }
  .matrix-cell.cell-pass { background: #d1e7dd; color: #0f5132; }
  .matrix-cell.cell-fail { background: #f8d7da; color: #842029; }
  .matrix-cell.cell-none { background: #e9ecef; color: #6c757d; }
  .matrix-cell.cell-active { outline: 2px solid #0d6efd; outline-offset: -2px; }

  /* Detail panel */
  #detail-panel { margin-top: 16px; }
  #detail-panel:empty { display: none; }
  .detail-box { border: 1px solid #dee2e6; border-radius: 8px; padding: 16px; background: #fff; }
  .detail-box h3 { font-size: 1rem; margin-bottom: 8px; }
  .detail-box .detail-meta { font-size: 0.85rem; color: #6c757d; margin-bottom: 12px; display: flex; gap: 16px; flex-wrap: wrap; }
  .step { padding: 6px 8px; margin: 4px 0; border-radius: 4px; display: flex; flex-wrap: wrap; gap: 8px; align-items: center; }
  .step.pass { background: #d1e7dd; }
  .step.fail { background: #f8d7da; }
  .step-name { font-weight: 500; }
  .step-duration { font-size: 0.85rem; color: #6c757d; margin-left: auto; }
  .attempts { width: 100%; font-size: 0.8rem; color: #6c757d; }
  .classification { width: 100%; font-size: 0.8rem; margin-top: 4px; }
  .badge { display: inline-block; padding: 2px 6px; border-radius: 3px; font-weight: 600; font-size: 0.7rem; text-transform: uppercase; margin-right: 6px; color: #fff; }
  .badge.agent { background: #6f42c1; }
  .badge.build { background: #fd7e14; }
  .badge.deploy { background: #0dcaf0; }
  .badge.assertion { background: #ffc107; color: #1a1a2e; }
  .badge.network { background: #20c997; }
  .badge.infra { background: #6c757d; }
  .badge.unknown { background: #adb5bd; }
  pre.error { width: 100%; background: #2b2d42; color: #edf2f4; padding: 8px; border-radius: 4px; font-size: 0.8rem; overflow-x: auto; white-space: pre-wrap; word-break: break-word; }

  /* Failures section */
  .failure-item { padding: 12px 0; border-bottom: 1px solid #dee2e6; }
  .failure-item:last-child { border-bottom: none; }
  .failure-header { display: flex; align-items: center; gap: 8px; margin-bottom: 4px; }
  .failure-scenario { font-size: 0.95rem; }
  .failure-meta { display: flex; gap: 16px; flex-wrap: wrap; font-size: 0.85rem; color: #6c757d; margin-bottom: 8px; }
`;

const JS = `
(function() {
  var data = window.__REPORT_DATA__;
  var filter = document.getElementById('language-filter');
  var table = document.getElementById('scenario-matrix-table');
  var detailPanel = document.getElementById('detail-panel');
  if (!table) return;

  var activeCell = null;

  function escHtml(s) {
    var d = document.createElement('div');
    d.textContent = s;
    return d.innerHTML;
  }

  function driverKey(r) {
    return r.matrix.model ? r.matrix.agent + '/' + r.matrix.model : r.matrix.agent;
  }

  function findReport(scenario, driver, lang) {
    for (var i = 0; i < data.length; i++) {
      var r = data[i];
      if (r.scenario === scenario && driverKey(r) === driver && (lang === '__all__' || r.matrix.language === lang)) {
        return r;
      }
    }
    return null;
  }

  function aggregateStatus(scenario, driver, lang) {
    var found = false;
    var anyFail = false;
    for (var i = 0; i < data.length; i++) {
      var r = data[i];
      if (r.scenario === scenario && driverKey(r) === driver) {
        if (lang === '__all__' || r.matrix.language === lang) {
          found = true;
          if (r.status === 'fail') anyFail = true;
        }
      }
    }
    if (!found) return 'none';
    return anyFail ? 'fail' : 'pass';
  }

  function updateCells() {
    var lang = filter ? filter.value : '__all__';
    var cells = table.querySelectorAll('.matrix-cell');
    for (var i = 0; i < cells.length; i++) {
      var cell = cells[i];
      var scenario = cell.getAttribute('data-scenario');
      var driver = cell.getAttribute('data-driver');
      var status = aggregateStatus(scenario, driver, lang);
      cell.className = 'matrix-cell cell-' + status;
      if (status === 'pass') cell.innerHTML = '\\u2713';
      else if (status === 'fail') cell.innerHTML = '\\u2717';
      else cell.innerHTML = '\\u2014';
    }
    // Clear detail panel and active state on filter change
    if (detailPanel) detailPanel.innerHTML = '';
    activeCell = null;
  }

  function renderStepName(step) {
    if (step.id) return escHtml(step.id);
    if (step.tag === 'prompt') {
      var p = step.prompt;
      var s = typeof p === 'string' ? p : JSON.stringify(p);
      return escHtml(s);
    }
    return 'step';
  }

  function showDetail(scenario, driver) {
    var lang = filter ? filter.value : '__all__';
    var matches = [];
    for (var i = 0; i < data.length; i++) {
      var r = data[i];
      if (r.scenario === scenario && driverKey(r) === driver) {
        if (lang === '__all__' || r.matrix.language === lang) {
          matches.push(r);
        }
      }
    }
    if (matches.length === 0) { detailPanel.innerHTML = ''; return; }

    var html = '';
    for (var m = 0; m < matches.length; m++) {
      var report = matches[m];
      html += '<div class="detail-box">';
      html += '<h3>' + escHtml(report.scenario) + '</h3>';
      html += '<div class="detail-meta">';
      html += '<span>Driver: <strong>' + escHtml(driverKey(report)) + '</strong></span>';
      html += '<span>Language: <strong>' + escHtml(report.matrix.language) + '</strong></span>';
      html += '<span>Status: <strong>' + report.status + '</strong></span>';
      html += '<span>Duration: <strong>' + report.durationSeconds.toFixed(1) + 's</strong></span>';
      html += '</div>';

      for (var j = 0; j < report.results.length; j++) {
        var r = report.results[j];
        var sClass = r.success ? 'pass' : 'fail';
        html += '<div class="step ' + sClass + '">';
        html += '<span class="step-name">' + renderStepName(r.step) + '</span>';
        html += '<span class="step-duration">' + r.durationSeconds.toFixed(1) + 's</span>';
        if (r.attempts && r.attempts.length > 0) {
          html += '<div class="attempts">Attempts: ';
          var parts = [];
          for (var a = 0; a < r.attempts.length; a++) {
            var at = r.attempts[a];
            parts.push('#' + at.attemptNumber + ' ' + (at.success ? 'pass' : 'fail') + ' (' + at.durationSeconds.toFixed(1) + 's)');
          }
          html += escHtml(parts.join(', '));
          html += '</div>';
        }
        if (r.error) {
          html += '<pre class="error">' + escHtml(r.error) + '</pre>';
        }
        if (r.classification) {
          html += '<div class="classification"><span class="badge ' + escHtml(r.classification.category) + '">' + escHtml(r.classification.category) + '</span> ' + escHtml(r.classification.guidance) + '</div>';
        }
        html += '</div>';
      }
      html += '</div>';
    }
    detailPanel.innerHTML = html;
  }

  if (filter) {
    filter.addEventListener('change', updateCells);
  }

  table.addEventListener('click', function(e) {
    var cell = e.target;
    while (cell && !cell.classList.contains('matrix-cell')) {
      if (cell === table) return;
      cell = cell.parentElement;
    }
    if (!cell) return;
    var scenario = cell.getAttribute('data-scenario');
    var driver = cell.getAttribute('data-driver');

    if (activeCell === cell) {
      cell.classList.remove('cell-active');
      detailPanel.innerHTML = '';
      activeCell = null;
      return;
    }
    if (activeCell) activeCell.classList.remove('cell-active');
    cell.classList.add('cell-active');
    activeCell = cell;
    showDetail(scenario, driver);
  });

  // Initial render
  updateCells();
})();
`;

export function generateHtmlReport(
  summary: Summary | MergedSummary,
  scenarioReports: ScenarioReport[],
): string {
  const title = isMergedSummary(summary) ? "Merged Test Report" : "Test Report";
  const matrixSection = isMergedSummary(summary) ? generateMatrixTable(summary) : "";

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
  ${generateScenarioMatrix(scenarioReports)}
  ${generateFailureSummary(summary, scenarioReports)}
  <script>
    window.__REPORT_DATA__ = ${JSON.stringify(scenarioReports)};
    ${JS}
  </script>
</body>
</html>`;
}

export type { ScenarioReport as HtmlScenarioReport };

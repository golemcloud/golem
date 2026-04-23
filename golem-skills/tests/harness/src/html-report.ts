import type { StepResult } from "./executor.js";
import type { UsageStats } from "./driver/base.js";

interface ScenarioReport {
  scenario: string;
  matrix: { agent: string; language: string; model?: string };
  run_id: string;
  status: "pass" | "fail";
  durationSeconds: number;
  results: StepResult[];
  artifactPaths: string[];
  usage?: UsageStats;
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
  scenarios: Array<{ name: string; status: string; durationSeconds: number; usage?: UsageStats }>;
  usage?: UsageStats;
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
    usage?: UsageStats;
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

function formatUsageCards(usage?: UsageStats): string {
  if (!usage) return "";
  const cards: string[] = [];
  if (usage.inputTokens || usage.outputTokens) {
    const inStr = (usage.inputTokens ?? 0).toLocaleString();
    const outStr = (usage.outputTokens ?? 0).toLocaleString();
    cards.push(
      `<div class="stat usage"><span class="label">Tokens</span><span class="value">${inStr} in / ${outStr} out</span></div>`,
    );
  }
  if (usage.costUsd) {
    cards.push(
      `<div class="stat usage"><span class="label">Cost</span><span class="value">$${usage.costUsd.toFixed(4)}</span></div>`,
    );
  }
  if (usage.numTurns) {
    cards.push(
      `<div class="stat usage"><span class="label">Turns</span><span class="value">${usage.numTurns}</span></div>`,
    );
  }
  return cards.join("\n        ");
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

  const usageCards = formatUsageCards(summary.usage);
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
        ${usageCards}
      </div>
    </div>`;
}

function passRateClass(passed: number, total: number): string {
  if (total === 0) return "rate-none";
  const pct = (passed / total) * 100;
  if (pct >= 75) return "rate-good";
  if (pct >= 40) return "rate-mid";
  return "rate-low";
}

function renderCard(entry: MergedSummary["heatMap"][number]): string {
  const pct = entry.total > 0 ? Math.round((entry.passed / entry.total) * 100) : 0;
  const rateClass = passRateClass(entry.passed, entry.total);
  const agent = escapeHtml(driverLabel(entry.agent, entry.model));

  const usageHtml = (() => {
    const u = entry.usage;
    if (!u) return "";
    const parts: string[] = [];
    if (u.inputTokens || u.outputTokens) {
      parts.push(
        `<span class="mu-item">🔤 ${(u.inputTokens ?? 0).toLocaleString()} / ${(u.outputTokens ?? 0).toLocaleString()}</span>`,
      );
    }
    if (u.costUsd) parts.push(`<span class="mu-item">💰 $${u.costUsd.toFixed(2)}</span>`);
    if (u.numTurns) parts.push(`<span class="mu-item">🔄 ${u.numTurns} turns</span>`);
    return parts.length > 0 ? `<div class="mu-row">${parts.join("")}</div>` : "";
  })();

  return `<div class="mx-card ${rateClass}">
    <div class="mx-header"><span class="mx-agent">${agent}</span></div>
    <div class="mx-bar-wrap"><div class="mx-bar" style="width:${pct}%"></div></div>
    <div class="mx-counts">
      <span class="mx-passed">✓ ${entry.passed}</span>
      <span class="mx-failed">✗ ${entry.failed}</span>
      <span class="mx-pct">${pct}%</span>
    </div>
    ${usageHtml}
  </div>`;
}

function generateMatrixTable(summary: MergedSummary): string {
  if (summary.heatMap.length === 0) return "";

  const languages = [...new Set(summary.heatMap.map((e) => e.language))].sort();
  const drivers = [...new Set(summary.heatMap.map((e) => driverLabel(e.agent, e.model)))].sort();

  const colHeaders = languages
    .map((l) => `<div class="mx-col-header">${escapeHtml(l)}</div>`)
    .join("\n");

  const rows = drivers
    .map((driver) => {
      const cells = languages
        .map((lang) => {
          const entry = summary.heatMap.find(
            (e) => driverLabel(e.agent, e.model) === driver && e.language === lang,
          );
          return entry ? renderCard(entry) : `<div class="mx-card mx-empty"></div>`;
        })
        .join("\n");
      return cells;
    })
    .join("\n");

  const colCount = languages.length;

  return `
    <div class="matrix-table">
      <h2>Matrix Results</h2>
      <div class="mx-grid" style="grid-template-columns: repeat(${colCount}, 1fr);">
        ${colHeaders}
        ${rows}
      </div>
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
    .flatMap((scenario) =>
      languages.map((language) => {
        const cells = drivers
          .map((driver) => {
            return `<td class="matrix-cell" data-scenario="${escapeHtml(scenario)}" data-driver="${escapeHtml(driver)}" data-language="${escapeHtml(language)}"></td>`;
          })
          .join("");
        return `<tr><td class="scenario-name">${escapeHtml(scenario)}</td><td class="lang-name">${escapeHtml(language)}</td>${cells}</tr>`;
      }),
    )
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
          <thead><tr><th class="scenario-col">Scenario</th><th class="lang-col">Language</th>${driverHeaders}</tr></thead>
          <tbody>${rows}</tbody>
        </table>
      </div>
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
    <details class="failure-summary">
      <summary><h2>Failures (${failures.length})</h2></summary>
      ${items}
    </details>`;
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
  .stat.usage .value { color: #6f42c1; font-size: 1rem; }
  .agent-labels { margin-top: 12px; font-size: 0.9rem; }
  .agent-labels .label { color: #6c757d; text-transform: uppercase; font-size: 0.75rem; margin-right: 4px; }
  .agent-label { display: inline-block; padding: 2px 8px; border-radius: 4px; background: #e9ecef; font-weight: 500; margin: 2px 4px; font-size: 0.85rem; }
  table { width: 100%; border-collapse: collapse; }
  th, td { padding: 8px 12px; text-align: left; border-bottom: 1px solid #dee2e6; }
  th { background: #e9ecef; font-size: 0.85rem; text-transform: uppercase; }

  /* Matrix result cards */
  .mx-grid { display: grid; gap: 12px; }
  .mx-col-header { font-size: 0.85rem; font-weight: 700; text-transform: uppercase; letter-spacing: 0.05em; color: #495057; text-align: center; padding-bottom: 4px; border-bottom: 2px solid #dee2e6; }
  .mx-card { border-radius: 8px; padding: 12px 14px; border: 1px solid #dee2e6; }
  .mx-card.rate-good { background: linear-gradient(135deg, #d1e7dd 0%, #f0faf4 100%); border-color: #a3cfbb; }
  .mx-card.rate-mid  { background: linear-gradient(135deg, #fff3cd 0%, #fffcf0 100%); border-color: #ffe69c; }
  .mx-card.rate-low  { background: linear-gradient(135deg, #f8d7da 0%, #fff5f5 100%); border-color: #f1aeb5; }
  .mx-card.rate-none { background: #e9ecef; }
  .mx-card.mx-empty { border: 1px dashed #dee2e6; background: transparent; }
  .mx-header { margin-bottom: 8px; }
  .mx-agent { font-weight: 600; font-size: 0.95rem; display: block; word-break: break-word; }
  .mx-bar-wrap { height: 6px; background: rgba(0,0,0,0.08); border-radius: 3px; overflow: hidden; margin-bottom: 6px; }
  .mx-bar { height: 100%; border-radius: 3px; transition: width 0.3s; }
  .rate-good .mx-bar { background: #198754; }
  .rate-mid  .mx-bar { background: #fd7e14; }
  .rate-low  .mx-bar { background: #dc3545; }
  .mx-counts { display: flex; gap: 12px; align-items: baseline; font-size: 0.85rem; }
  .mx-passed { color: #198754; font-weight: 600; }
  .mx-failed { color: #dc3545; font-weight: 600; }
  .mx-pct { margin-left: auto; font-weight: 700; font-size: 1.1rem; }
  .rate-good .mx-pct { color: #198754; }
  .rate-mid  .mx-pct { color: #b86e00; }
  .rate-low  .mx-pct { color: #dc3545; }
  .mu-row { display: flex; flex-wrap: wrap; gap: 8px; margin-top: 6px; font-size: 0.78rem; color: #6c757d; }
  .mu-item { white-space: nowrap; }

  /* Scenario matrix */
  .matrix-controls { margin-bottom: 12px; display: flex; align-items: center; gap: 8px; }
  .matrix-controls label { font-size: 0.85rem; font-weight: 500; }
  .matrix-controls select { padding: 4px 8px; border-radius: 4px; border: 1px solid #ced4da; font-size: 0.85rem; }
  .matrix-table-wrap { overflow-x: auto; }
  .interactive-matrix th.scenario-col { min-width: 200px; }
  .interactive-matrix th.lang-col { min-width: 70px; }
  .interactive-matrix .lang-name { font-size: 0.85rem; color: #6c757d; }
  .interactive-matrix th.driver-col { text-align: center; min-width: 90px; font-size: 0.75rem; }
  .matrix-cell { text-align: center; cursor: pointer; font-weight: bold; font-size: 1rem; transition: opacity 0.15s; user-select: none; }
  .matrix-cell:hover { opacity: 0.75; }
  .matrix-cell.cell-pass { background: #d1e7dd; color: #0f5132; }
  .matrix-cell.cell-fail { background: #f8d7da; color: #842029; }
  .matrix-cell.cell-none { background: #e9ecef; color: #6c757d; }
  .matrix-cell.cell-active { outline: 2px solid #0d6efd; outline-offset: -2px; }

  /* Inline detail row */
  .detail-row td { padding: 0; border-bottom: 1px solid #dee2e6; }
  .detail-box { border: 1px solid #dee2e6; border-radius: 8px; padding: 16px; margin: 8px; background: #f8f9fa; }
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
  .failure-summary > summary { cursor: pointer; list-style: none; }
  .failure-summary > summary::-webkit-details-marker { display: none; }
  .failure-summary > summary h2::before { content: '▶ '; font-size: 0.8rem; }
  .failure-summary[open] > summary h2::before { content: '▼ '; }
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
  if (!table) return;

  var activeCell = null;
  var detailRow = null;

  function escHtml(s) {
    var d = document.createElement('div');
    d.textContent = s;
    return d.innerHTML;
  }

  function driverKey(r) {
    return r.matrix.model ? r.matrix.agent + '/' + r.matrix.model : r.matrix.agent;
  }

  function cellStatus(scenario, driver, lang) {
    for (var i = 0; i < data.length; i++) {
      var r = data[i];
      if (r.scenario === scenario && driverKey(r) === driver && r.matrix.language === lang) {
        return r.status === 'fail' ? 'fail' : 'pass';
      }
    }
    return 'none';
  }

  function removeDetail() {
    if (detailRow) { detailRow.remove(); detailRow = null; }
    if (activeCell) { activeCell.classList.remove('cell-active'); activeCell = null; }
  }

  function updateCells() {
    var cells = table.querySelectorAll('.matrix-cell');
    for (var i = 0; i < cells.length; i++) {
      var cell = cells[i];
      var scenario = cell.getAttribute('data-scenario');
      var driver = cell.getAttribute('data-driver');
      var lang = cell.getAttribute('data-language');
      var status = cellStatus(scenario, driver, lang);
      cell.className = 'matrix-cell cell-' + status;
      if (status === 'pass') cell.innerHTML = '\\u2713';
      else if (status === 'fail') cell.innerHTML = '\\u2717';
      else cell.innerHTML = '\\u2014';
    }
    removeDetail();
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

  function renderDetailHtml(scenario, driver, lang) {
    var report = null;
    for (var i = 0; i < data.length; i++) {
      var r = data[i];
      if (r.scenario === scenario && driverKey(r) === driver && r.matrix.language === lang) {
        report = r;
        break;
      }
    }
    if (!report) return '';

    var html = '<div class="detail-box">';
    html += '<h3>' + escHtml(report.scenario) + '</h3>';
    html += '<div class="detail-meta">';
    html += '<span>Driver: <strong>' + escHtml(driverKey(report)) + '</strong></span>';
    html += '<span>Language: <strong>' + escHtml(report.matrix.language) + '</strong></span>';
    html += '<span>Status: <strong>' + report.status + '</strong></span>';
    html += '<span>Duration: <strong>' + report.durationSeconds.toFixed(1) + 's</strong></span>';
    if (report.usage) {
      var u = report.usage;
      if (u.inputTokens || u.outputTokens) html += '<span>Tokens: <strong>' + (u.inputTokens||0).toLocaleString() + ' in / ' + (u.outputTokens||0).toLocaleString() + ' out</strong></span>';
      if (u.costUsd) html += '<span>Cost: <strong>$' + u.costUsd.toFixed(4) + '</strong></span>';
      if (u.numTurns) html += '<span>Turns: <strong>' + u.numTurns + '</strong></span>';
    }
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
    return html;
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
    var lang = cell.getAttribute('data-language');

    if (activeCell === cell) {
      removeDetail();
      return;
    }
    removeDetail();

    var html = renderDetailHtml(scenario, driver, lang);
    if (!html) return;

    cell.classList.add('cell-active');
    activeCell = cell;

    var row = cell.closest('tr');
    var colCount = row.children.length;
    detailRow = document.createElement('tr');
    detailRow.className = 'detail-row';
    var td = document.createElement('td');
    td.setAttribute('colspan', colCount);
    td.innerHTML = html;
    detailRow.appendChild(td);
    row.parentNode.insertBefore(detailRow, row.nextSibling);
  });

  function filterRows() {
    var lang = filter ? filter.value : '__all__';
    removeDetail();
    var rows = table.tBodies[0].rows;
    for (var i = 0; i < rows.length; i++) {
      var row = rows[i];
      var cell = row.querySelector('.matrix-cell');
      if (!cell) continue;
      var rowLang = cell.getAttribute('data-language');
      row.style.display = (lang === '__all__' || rowLang === lang) ? '' : 'none';
    }
  }

  if (filter) {
    filter.addEventListener('change', filterRows);
  }

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

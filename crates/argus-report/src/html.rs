// Copyright 2026 Hans W. Uhlig
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Rich standalone HTML report export with interactive filtering and styling.

use crate::differential::{DifferentialFinding, DifferentialReport, FindingCategory};
use argus_core::{RunId, Severity};
use std::fmt::Write as _;

/// Escape standard HTML entities to prevent XSS and formatting breakage.
#[must_use]
pub fn escape_html(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

/// Standalone HTML report options.
#[derive(Clone, Debug, Default)]
pub struct HtmlReportOptions {
    /// Custom page title.
    pub title: Option<String>,
    /// Custom subtitle or organization header.
    pub subtitle: Option<String>,
    /// Baseline run identifier if this is a comparative report.
    pub baseline_run_id: Option<RunId>,
}

/// Render a self-contained HTML report from a differential review report.
#[must_use]
pub fn render_differential_html_report(report: &DifferentialReport) -> String {
    let mut all_findings = Vec::with_capacity(
        report.new_findings.len()
            + report.persistent_findings.len()
            + report.resolved_findings.len(),
    );
    all_findings.extend(report.new_findings.clone());
    all_findings.extend(report.persistent_findings.clone());
    all_findings.extend(report.resolved_findings.clone());

    let options = HtmlReportOptions {
        title: Some(format!(
            "Argus Differential Review: {} vs {}",
            report.current_run_id, report.baseline_run_id
        )),
        subtitle: Some(format!(
            "Baseline Run: {} | Current Run: {}",
            report.baseline_run_id, report.current_run_id
        )),
        baseline_run_id: Some(report.baseline_run_id.clone()),
    };

    render_findings_html_report(&report.current_run_id, &all_findings, &options)
}

/// Render a self-contained HTML report from a set of normalized findings.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn render_findings_html_report(
    run_id: &RunId,
    findings: &[DifferentialFinding],
    options: &HtmlReportOptions,
) -> String {
    let page_title = options
        .title
        .as_deref()
        .unwrap_or("Argus Intelligent Code Review Report");
    let subtitle = options
        .subtitle
        .as_deref()
        .unwrap_or("Comprehensive Automated Review Findings");

    let mut crit_count = 0;
    let mut high_count = 0;
    let mut med_count = 0;
    let mut low_count = 0;
    let mut note_count = 0;

    let mut new_count = 0;
    let mut resolved_count = 0;
    let mut persistent_count = 0;

    for f in findings {
        match f.severity {
            Severity::Critical => crit_count += 1,
            Severity::High => high_count += 1,
            Severity::Medium => med_count += 1,
            Severity::Low => low_count += 1,
            Severity::Note => note_count += 1,
        }
        match f.category {
            FindingCategory::New => new_count += 1,
            FindingCategory::Resolved => resolved_count += 1,
            FindingCategory::Persistent => persistent_count += 1,
        }
    }

    let is_differential = options.baseline_run_id.is_some() || resolved_count > 0;

    let mut html = String::with_capacity(64 * 1024);
    let _ = write!(
        html,
        r#"<!DOCTYPE html>
<html lang="en" data-theme="dark">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>{} - {}</title>
  <style>
    :root {{
      --font-sans: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
      --font-mono: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", monospace;

      --bg: #0b0f19;
      --bg-card: #151d30;
      --bg-card-header: #1c2740;
      --bg-hover: #22304e;
      --bg-input: #0b0f19;
      --border: #263554;
      --border-focus: #38bdf8;
      --text: #f8fafc;
      --text-muted: #94a3b8;
      --text-dim: #64748b;

      --crit: #ef4444;
      --crit-bg: rgba(239, 68, 68, 0.15);
      --crit-border: rgba(239, 68, 68, 0.4);
      --high: #f97316;
      --high-bg: rgba(249, 115, 22, 0.15);
      --high-border: rgba(249, 115, 22, 0.4);
      --med: #eab308;
      --med-bg: rgba(234, 179, 8, 0.15);
      --med-border: rgba(234, 179, 8, 0.4);
      --low: #38bdf8;
      --low-bg: rgba(56, 189, 248, 0.15);
      --low-border: rgba(56, 189, 248, 0.4);
      --note: #94a3b8;
      --note-bg: rgba(148, 163, 184, 0.15);
      --note-border: rgba(148, 163, 184, 0.4);

      --cat-new: #f43f5e;
      --cat-new-bg: rgba(244, 63, 94, 0.2);
      --cat-res: #10b981;
      --cat-res-bg: rgba(16, 185, 129, 0.2);
      --cat-per: #94a3b8;
      --cat-per-bg: rgba(148, 163, 184, 0.15);

      --accent: #38bdf8;
      --accent-glow: rgba(56, 189, 248, 0.25);
    }}

    [data-theme="light"] {{
      --bg: #f8fafc;
      --bg-card: #ffffff;
      --bg-card-header: #f1f5f9;
      --bg-hover: #e2e8f0;
      --bg-input: #ffffff;
      --border: #cbd5e1;
      --border-focus: #0284c7;
      --text: #0f172a;
      --text-muted: #475569;
      --text-dim: #94a3b8;

      --crit: #dc2626;
      --crit-bg: #fee2e2;
      --crit-border: #fca5a5;
      --high: #ea580c;
      --high-bg: #ffedd5;
      --high-border: #fdba74;
      --med: #d97706;
      --med-bg: #fef3c7;
      --med-border: #fde68a;
      --low: #0284c7;
      --low-bg: #e0f2fe;
      --low-border: #bae6fd;
      --note: #64748b;
      --note-bg: #f1f5f9;
      --note-border: #cbd5e1;

      --cat-new: #e11d48;
      --cat-new-bg: #ffe4e6;
      --cat-res: #059669;
      --cat-res-bg: #d1fae5;
      --cat-per: #64748b;
      --cat-per-bg: #f1f5f9;

      --accent: #0284c7;
      --accent-glow: rgba(2, 132, 199, 0.15);
    }}

    * {{ box-sizing: border-box; margin: 0; padding: 0; }}
    body {{
      font-family: var(--font-sans);
      background-color: var(--bg);
      color: var(--text);
      line-height: 1.5;
      padding: 32px 16px;
      transition: background-color 0.2s ease, color 0.2s ease;
    }}
    .container {{
      max-width: 1280px;
      margin: 0 auto;
    }}

    /* Header */
    .header {{
      display: flex;
      justify-content: space-between;
      align-items: flex-start;
      margin-bottom: 28px;
      padding-bottom: 24px;
      border-bottom: 1px solid var(--border);
      flex-wrap: wrap;
      gap: 16px;
    }}
    .header-left {{
      display: flex;
      align-items: center;
      gap: 16px;
    }}
    .logo-badge {{
      width: 44px;
      height: 44px;
      border-radius: 12px;
      background: linear-gradient(135deg, #38bdf8, #818cf8);
      display: flex;
      align-items: center;
      justify-content: center;
      box-shadow: 0 0 16px var(--accent-glow);
    }}
    .logo-badge svg {{
      width: 26px;
      height: 26px;
      fill: #ffffff;
    }}
    .header-title h1 {{
      font-size: 1.6rem;
      font-weight: 700;
      letter-spacing: -0.02em;
    }}
    .header-title p {{
      color: var(--text-muted);
      font-size: 0.9rem;
      margin-top: 2px;
    }}
    .header-meta {{
      display: flex;
      align-items: center;
      gap: 12px;
      flex-wrap: wrap;
    }}
    .tag {{
      display: inline-flex;
      align-items: center;
      font-family: var(--font-mono);
      font-size: 0.8rem;
      padding: 4px 10px;
      border-radius: 6px;
      background-color: var(--bg-card);
      border: 1px solid var(--border);
      color: var(--text-muted);
    }}
    .btn {{
      display: inline-flex;
      align-items: center;
      gap: 6px;
      background-color: var(--bg-card);
      color: var(--text);
      border: 1px solid var(--border);
      padding: 8px 14px;
      border-radius: 8px;
      cursor: pointer;
      font-size: 0.85rem;
      font-weight: 500;
      transition: background-color 0.15s ease, border-color 0.15s ease;
    }}
    .btn:hover {{
      background-color: var(--bg-hover);
      border-color: var(--border-focus);
    }}

    /* Metrics Grid */
    .metrics-grid {{
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(160px, 1fr));
      gap: 14px;
      margin-bottom: 28px;
    }}
    .metric-card {{
      background-color: var(--bg-card);
      border: 1px solid var(--border);
      border-radius: 10px;
      padding: 16px;
      display: flex;
      flex-direction: column;
      gap: 6px;
      transition: transform 0.15s ease, border-color 0.15s ease;
    }}
    .metric-card:hover {{
      transform: translateY(-2px);
      border-color: var(--border-focus);
    }}
    .metric-title {{
      font-size: 0.8rem;
      font-weight: 600;
      text-transform: uppercase;
      letter-spacing: 0.05em;
      color: var(--text-muted);
    }}
    .metric-value {{
      font-size: 1.8rem;
      font-weight: 700;
      line-height: 1;
      font-family: var(--font-mono);
    }}
    .metric-card.critical .metric-value {{ color: var(--crit); }}
    .metric-card.high .metric-value {{ color: var(--high); }}
    .metric-card.medium .metric-value {{ color: var(--med); }}
    .metric-card.low .metric-value {{ color: var(--low); }}
    .metric-card.note .metric-value {{ color: var(--note); }}
    .metric-card.cat-new .metric-value {{ color: var(--cat-new); }}
    .metric-card.cat-resolved .metric-value {{ color: var(--cat-res); }}
    .metric-card.cat-persistent .metric-value {{ color: var(--cat-per); }}

    /* Controls & Filtering Toolbar */
    .toolbar {{
      background-color: var(--bg-card);
      border: 1px solid var(--border);
      border-radius: 12px;
      padding: 16px;
      margin-bottom: 24px;
      display: flex;
      flex-direction: column;
      gap: 14px;
    }}
    .toolbar-row {{
      display: flex;
      flex-wrap: wrap;
      gap: 12px;
      align-items: center;
      justify-content: space-between;
    }}
    .search-box {{
      flex: 1;
      min-width: 260px;
      position: relative;
    }}
    .search-box input {{
      width: 100%;
      background-color: var(--bg-input);
      border: 1px solid var(--border);
      border-radius: 8px;
      padding: 10px 14px 10px 38px;
      color: var(--text);
      font-size: 0.9rem;
      outline: none;
      transition: border-color 0.15s ease;
    }}
    .search-box input:focus {{
      border-color: var(--border-focus);
      box-shadow: 0 0 0 3px var(--accent-glow);
    }}
    .search-icon {{
      position: absolute;
      left: 12px;
      top: 50%;
      transform: translateY(-50%);
      width: 16px;
      height: 16px;
      fill: var(--text-dim);
      pointer-events: none;
    }}
    .filter-group {{
      display: flex;
      flex-wrap: wrap;
      gap: 8px;
      align-items: center;
    }}
    .filter-label {{
      font-size: 0.8rem;
      font-weight: 600;
      color: var(--text-muted);
      margin-right: 4px;
    }}
    .pill {{
      padding: 5px 12px;
      border-radius: 20px;
      font-size: 0.8rem;
      font-weight: 500;
      cursor: pointer;
      background-color: var(--bg-input);
      border: 1px solid var(--border);
      color: var(--text-muted);
      display: inline-flex;
      align-items: center;
      gap: 6px;
      user-select: none;
      transition: all 0.15s ease;
    }}
    .pill:hover {{
      background-color: var(--bg-hover);
      color: var(--text);
    }}
    .pill.active {{
      background-color: var(--accent);
      color: #ffffff;
      border-color: var(--accent);
    }}
    .pill .badge {{
      font-family: var(--font-mono);
      font-size: 0.75rem;
      padding: 1px 6px;
      border-radius: 10px;
      background: rgba(0, 0, 0, 0.25);
    }}
    .status-bar {{
      display: flex;
      justify-content: space-between;
      align-items: center;
      padding: 0 4px;
      font-size: 0.85rem;
      color: var(--text-muted);
    }}

    /* Findings List */
    .findings-list {{
      display: flex;
      flex-direction: column;
      gap: 12px;
    }}
    .finding-card {{
      background-color: var(--bg-card);
      border: 1px solid var(--border);
      border-radius: 10px;
      overflow: hidden;
      transition: border-color 0.15s ease, box-shadow 0.15s ease;
    }}
    .finding-card:hover {{
      border-color: var(--border-focus);
    }}
    .finding-card.critical {{ border-left: 4px solid var(--crit); }}
    .finding-card.high {{ border-left: 4px solid var(--high); }}
    .finding-card.medium {{ border-left: 4px solid var(--med); }}
    .finding-card.low {{ border-left: 4px solid var(--low); }}
    .finding-card.note {{ border-left: 4px solid var(--note); }}

    .finding-header {{
      padding: 14px 18px;
      background-color: var(--bg-card-header);
      cursor: pointer;
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 16px;
      user-select: none;
    }}
    .finding-header-left {{
      display: flex;
      align-items: center;
      gap: 10px;
      flex-wrap: wrap;
      flex: 1;
    }}
    .badge-sev {{
      font-size: 0.72rem;
      font-weight: 700;
      text-transform: uppercase;
      padding: 3px 8px;
      border-radius: 4px;
      letter-spacing: 0.05em;
    }}
    .badge-sev.critical {{ background: var(--crit-bg); color: var(--crit); border: 1px solid var(--crit-border); }}
    .badge-sev.high {{ background: var(--high-bg); color: var(--high); border: 1px solid var(--high-border); }}
    .badge-sev.medium {{ background: var(--med-bg); color: var(--med); border: 1px solid var(--med-border); }}
    .badge-sev.low {{ background: var(--low-bg); color: var(--low); border: 1px solid var(--low-border); }}
    .badge-sev.note {{ background: var(--note-bg); color: var(--note); border: 1px solid var(--note-border); }}

    .badge-cat {{
      font-size: 0.72rem;
      font-weight: 600;
      padding: 3px 8px;
      border-radius: 4px;
    }}
    .badge-cat.new {{ background: var(--cat-new-bg); color: var(--cat-new); }}
    .badge-cat.resolved {{ background: var(--cat-res-bg); color: var(--cat-res); }}
    .badge-cat.persistent {{ background: var(--cat-per-bg); color: var(--cat-per); }}

    .badge-policy {{
      font-size: 0.75rem;
      font-family: var(--font-mono);
      color: var(--accent);
      background: var(--accent-glow);
      padding: 2px 8px;
      border-radius: 4px;
    }}
    .finding-title {{
      font-size: 0.95rem;
      font-weight: 600;
      color: var(--text);
    }}
    .finding-location-preview {{
      font-family: var(--font-mono);
      font-size: 0.8rem;
      color: var(--text-dim);
      margin-left: auto;
    }}
    .chevron {{
      width: 18px;
      height: 18px;
      fill: var(--text-muted);
      transition: transform 0.2s ease;
      flex-shrink: 0;
    }}
    .finding-card.open .chevron {{
      transform: rotate(180deg);
    }}

    .finding-body {{
      display: none;
      padding: 18px;
      border-top: 1px solid var(--border);
      background-color: var(--bg-card);
      display: flex;
      flex-direction: column;
      gap: 16px;
    }}
    .finding-card:not(.open) .finding-body {{
      display: none !important;
    }}

    .finding-desc {{
      font-size: 0.9rem;
      color: var(--text);
      white-space: pre-wrap;
    }}

    .meta-table {{
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
      gap: 12px;
      background: var(--bg-input);
      border: 1px solid var(--border);
      border-radius: 8px;
      padding: 14px;
    }}
    .meta-item {{
      display: flex;
      flex-direction: column;
      gap: 4px;
    }}
    .meta-item-label {{
      font-size: 0.75rem;
      text-transform: uppercase;
      font-weight: 600;
      letter-spacing: 0.05em;
      color: var(--text-muted);
    }}
    .meta-item-val {{
      font-size: 0.85rem;
      font-family: var(--font-mono);
      color: var(--text);
      word-break: break-all;
    }}

    .pill-tag {{
      display: inline-block;
      font-size: 0.75rem;
      background: var(--bg-hover);
      color: var(--text-muted);
      padding: 2px 8px;
      border-radius: 4px;
      margin-right: 4px;
      margin-bottom: 4px;
    }}

    .empty-state {{
      text-align: center;
      padding: 48px 16px;
      background-color: var(--bg-card);
      border: 1px dashed var(--border);
      border-radius: 12px;
      color: var(--text-muted);
    }}

    /* Footer */
    .footer {{
      margin-top: 48px;
      padding-top: 24px;
      border-top: 1px solid var(--border);
      text-align: center;
      font-size: 0.8rem;
      color: var(--text-dim);
    }}
  </style>
</head>
<body>
  <div class="container">
    <header class="header">
      <div class="header-left">
        <div class="logo-badge">
          <svg viewBox="0 0 24 24">
            <path d="M12 4.5C7 4.5 2.73 7.61 1 12c1.73 4.39 6 7.5 11 7.5s9.27-3.11 11-7.5c-1.73-4.39-6-7.5-11-7.5zM12 17c-2.76 0-5-2.24-5-5s2.24-5 5-5 5 2.24 5 5-2.24 5-5 5zm0-8c-1.66 0-3 1.34-3 3s1.34 3 3 3 3-1.34 3-3-1.34-3-3-3z"/>
          </svg>
        </div>
        <div class="header-title">
          <h1>{}</h1>
          <p>{}</p>
        </div>
      </div>
      <div class="header-meta">
        <span class="tag">Run: {}</span>
"#,
        escape_html(page_title),
        run_id,
        escape_html(page_title),
        escape_html(subtitle),
        run_id,
    );

    if let Some(ref base_id) = options.baseline_run_id {
        let _ = write!(
            html,
            r#"        <span class="tag">Baseline: {}</span>
"#,
            base_id
        );
    }

    let _ = write!(
        html,
        r#"        <button class="btn" id="theme-toggle" title="Toggle color theme">
          <span id="theme-icon">☀️</span> Theme
        </button>
      </div>
    </header>

    <!-- Metrics Cards -->
    <section class="metrics-grid">
      <div class="metric-card">
        <div class="metric-title">Total Findings</div>
        <div class="metric-value">{}</div>
      </div>
      <div class="metric-card critical">
        <div class="metric-title">Critical</div>
        <div class="metric-value">{}</div>
      </div>
      <div class="metric-card high">
        <div class="metric-title">High</div>
        <div class="metric-value">{}</div>
      </div>
      <div class="metric-card medium">
        <div class="metric-title">Medium</div>
        <div class="metric-value">{}</div>
      </div>
      <div class="metric-card low">
        <div class="metric-title">Low</div>
        <div class="metric-value">{}</div>
      </div>
      <div class="metric-card note">
        <div class="metric-title">Note</div>
        <div class="metric-value">{}</div>
      </div>
"#,
        findings.len(),
        crit_count,
        high_count,
        med_count,
        low_count,
        note_count
    );

    if is_differential {
        let _ = write!(
            html,
            r#"      <div class="metric-card cat-new">
        <div class="metric-title">New Findings</div>
        <div class="metric-value">{}</div>
      </div>
      <div class="metric-card cat-resolved">
        <div class="metric-title">Resolved</div>
        <div class="metric-value">{}</div>
      </div>
      <div class="metric-card cat-persistent">
        <div class="metric-title">Persistent</div>
        <div class="metric-value">{}</div>
      </div>
"#,
            new_count, resolved_count, persistent_count
        );
    }

    let _ = write!(
        html,
        r#"    </section>

    <!-- Filter Toolbar -->
    <section class="toolbar">
      <div class="toolbar-row">
        <div class="search-box">
          <svg class="search-icon" viewBox="0 0 24 24">
            <path d="M15.5 14h-.79l-.28-.27A6.471 6.471 0 0 0 16 9.5 6.5 6.5 0 1 0 9.5 16c1.61 0 3.09-.59 4.23-1.57l.27.28v.79l5 4.99L20.49 19l-4.99-5zm-6 0C7.01 14 5 11.99 5 9.5S7.01 5 9.5 5 14 7.01 14 9.5 11.99 14 9.5 14z"/>
          </svg>
          <input type="search" id="search-input" placeholder="Search findings by title, path, target, or content...">
        </div>
        <div>
          <button class="btn" id="toggle-expand-all">Expand All</button>
        </div>
      </div>

      <div class="toolbar-row">
        <div class="filter-group">
          <span class="filter-label">Severity:</span>
          <span class="pill active" data-filter-type="severity" data-filter-val="all">All</span>
          <span class="pill" data-filter-type="severity" data-filter-val="critical">Critical <span class="badge">{}</span></span>
          <span class="pill" data-filter-type="severity" data-filter-val="high">High <span class="badge">{}</span></span>
          <span class="pill" data-filter-type="severity" data-filter-val="medium">Med <span class="badge">{}</span></span>
          <span class="pill" data-filter-type="severity" data-filter-val="low">Low <span class="badge">{}</span></span>
          <span class="pill" data-filter-type="severity" data-filter-val="note">Note <span class="badge">{}</span></span>
        </div>
"#,
        crit_count, high_count, med_count, low_count, note_count
    );

    if is_differential {
        let _ = write!(
            html,
            r#"        <div class="filter-group">
          <span class="filter-label">Status:</span>
          <span class="pill active" data-filter-type="category" data-filter-val="all">All</span>
          <span class="pill" data-filter-type="category" data-filter-val="new">New <span class="badge">{}</span></span>
          <span class="pill" data-filter-type="category" data-filter-val="resolved">Resolved <span class="badge">{}</span></span>
          <span class="pill" data-filter-type="category" data-filter-val="persistent">Persistent <span class="badge">{}</span></span>
        </div>
"#,
            new_count, resolved_count, persistent_count
        );
    }

    let _ = write!(
        html,
        r#"        <div class="filter-group">
          <span class="filter-label">Policy:</span>
          <span class="pill active" data-filter-type="policy" data-filter-val="all">All</span>
          <span class="pill" data-filter-type="policy" data-filter-val="documentation">Docs</span>
          <span class="pill" data-filter-type="policy" data-filter-val="correctness">Correctness</span>
          <span class="pill" data-filter-type="policy" data-filter-val="architecture">Arch</span>
          <span class="pill" data-filter-type="policy" data-filter-val="conformance">Conformance</span>
          <span class="pill" data-filter-type="policy" data-filter-val="maintainability">Maintainability</span>
          <span class="pill" data-filter-type="policy" data-filter-val="optimization">Optimization</span>
        </div>
      </div>

      <div class="status-bar">
        <span>Showing <strong id="visible-count">{}</strong> of <strong id="total-count">{}</strong> findings</span>
      </div>
    </section>

    <!-- Findings Section -->
    <main class="findings-list" id="findings-container">
"#,
        findings.len(),
        findings.len()
    );

    if findings.is_empty() {
        html.push_str(
            r#"      <div class="empty-state">
        <h3>No findings surfaced</h3>
        <p>This audit run produced zero findings matching the selected policies.</p>
      </div>
"#,
        );
    } else {
        for finding in findings {
            let sev_str = match finding.severity {
                Severity::Critical => "critical",
                Severity::High => "high",
                Severity::Medium => "medium",
                Severity::Low => "low",
                Severity::Note => "note",
            };
            let cat_str = match finding.category {
                FindingCategory::New => "new",
                FindingCategory::Resolved => "resolved",
                FindingCategory::Persistent => "persistent",
            };
            let cat_badge_text = match finding.category {
                FindingCategory::New => "NEW",
                FindingCategory::Resolved => "RESOLVED",
                FindingCategory::Persistent => "PERSISTENT",
            };
            let pol_lower = finding.policy.to_lowercase();
            let targets_str = finding
                .targets
                .iter()
                .map(|t| t.to_string())
                .collect::<Vec<_>>()
                .join(" ");

            let search_text = format!(
                "{} {} {} {} {} {}",
                finding.title.to_lowercase(),
                finding.description.to_lowercase(),
                finding
                    .primary_location
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase(),
                finding.policy.to_lowercase(),
                targets_str.to_lowercase(),
                finding.dimensions.join(" ").to_lowercase()
            );

            let _ = write!(
                html,
                r#"      <article class="finding-card {sev_str}" data-severity="{sev_str}" data-category="{cat_str}" data-policy="{pol_lower}" data-search="{search_text}">
        <div class="finding-header" onclick="toggleCard(this.parentElement)">
          <div class="finding-header-left">
            <span class="badge-sev {sev_str}">{sev_str}</span>
"#
            );

            if is_differential {
                let _ = write!(
                    html,
                    r#"            <span class="badge-cat {cat_str}">{cat_badge_text}</span>
"#
                );
            }

            let _ = write!(
                html,
                r#"            <span class="badge-policy">{}</span>
            <span class="finding-title">{}</span>
          </div>
"#,
                escape_html(&finding.policy),
                escape_html(&finding.title),
            );

            if let Some(ref loc) = finding.primary_location {
                let _ = write!(
                    html,
                    r#"          <span class="finding-location-preview">{}</span>
"#,
                    escape_html(loc)
                );
            }

            let _ = write!(
                html,
                r#"          <svg class="chevron" viewBox="0 0 24 24">
            <path d="M7.41 8.59L12 13.17l4.59-4.58L18 10l-6 6-6-6 1.41-1.41z"/>
          </svg>
        </div>
        <div class="finding-body">
          <div class="finding-desc">{}</div>
          <div class="meta-table">
            <div class="meta-item">
              <span class="meta-item-label">Cluster Finding ID</span>
              <span class="meta-item-val">{}</span>
            </div>
            <div class="meta-item">
              <span class="meta-item-label">Adjudication State</span>
              <span class="meta-item-val">{:?}</span>
            </div>
            <div class="meta-item">
              <span class="meta-item-label">Confidence Score</span>
              <span class="meta-item-val">{:.1}%</span>
            </div>
"#,
                escape_html(&finding.description),
                finding.id,
                finding.adjudication,
                f64::from(finding.confidence.basis_points()) / 100.0,
            );

            if let Some(ref loc) = finding.primary_location {
                let _ = write!(
                    html,
                    r#"            <div class="meta-item">
              <span class="meta-item-label">Location</span>
              <span class="meta-item-val">{}</span>
            </div>
"#,
                    escape_html(loc)
                );
            }

            if !finding.targets.is_empty() {
                let targets_tags = finding
                    .targets
                    .iter()
                    .map(|t| {
                        format!(
                            r#"<span class="pill-tag">{}</span>"#,
                            escape_html(t.as_str())
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("");
                let _ = write!(
                    html,
                    r#"            <div class="meta-item">
              <span class="meta-item-label">Targets</span>
              <div>{}</div>
            </div>
"#,
                    targets_tags
                );
            }

            if !finding.dimensions.is_empty() {
                let dim_tags = finding
                    .dimensions
                    .iter()
                    .map(|d| format!(r#"<span class="pill-tag">{}</span>"#, escape_html(d)))
                    .collect::<Vec<_>>()
                    .join("");
                let _ = write!(
                    html,
                    r#"            <div class="meta-item">
              <span class="meta-item-label">Dimensions</span>
              <div>{}</div>
            </div>
"#,
                    dim_tags
                );
            }

            if let Some(ref base_id) = finding.baseline_id {
                let _ = write!(
                    html,
                    r#"            <div class="meta-item">
              <span class="meta-item-label">Matched Baseline Finding</span>
              <span class="meta-item-val">{}</span>
            </div>
"#,
                    base_id
                );
            }

            html.push_str(
                r#"          </div>
        </div>
      </article>
"#,
            );
        }
    }

    let _ = write!(
        html,
        r#"    </main>

    <footer class="footer">
      Generated by Argus Intelligent Source Code Review Harness &bull; Run: <code>{}</code>
    </footer>
  </div>

  <script>
    (function() {{
      const state = {{
        severity: 'all',
        category: 'all',
        policy: 'all',
        search: '',
        allExpanded: false,
      }};

      // Theme toggle
      const themeToggle = document.getElementById('theme-toggle');
      const themeIcon = document.getElementById('theme-icon');
      const savedTheme = localStorage.getItem('argus-theme') || 
        (window.matchMedia && window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark');
      setTheme(savedTheme);

      themeToggle.addEventListener('click', () => {{
        const cur = document.documentElement.getAttribute('data-theme') || 'dark';
        const next = cur === 'dark' ? 'light' : 'dark';
        setTheme(next);
      }});

      function setTheme(theme) {{
        document.documentElement.setAttribute('data-theme', theme);
        localStorage.setItem('argus-theme', theme);
        if (themeIcon) {{
          themeIcon.textContent = theme === 'light' ? '🌙' : '☀️';
        }}
      }}

      // Card Accordion
      window.toggleCard = function(card) {{
        card.classList.toggle('open');
      }};

      // Expand / Collapse All
      const toggleExpandBtn = document.getElementById('toggle-expand-all');
      toggleExpandBtn.addEventListener('click', () => {{
        state.allExpanded = !state.allExpanded;
        const cards = document.querySelectorAll('.finding-card');
        cards.forEach(c => {{
          if (state.allExpanded) {{
            c.classList.add('open');
          }} else {{
            c.classList.remove('open');
          }}
        }});
        toggleExpandBtn.textContent = state.allExpanded ? 'Collapse All' : 'Expand All';
      }});

      // Filtering logic
      const searchInput = document.getElementById('search-input');
      const pills = document.querySelectorAll('.pill');
      const visibleCountEl = document.getElementById('visible-count');
      const cards = Array.from(document.querySelectorAll('.finding-card'));

      pills.forEach(pill => {{
        pill.addEventListener('click', () => {{
          const filterType = pill.getAttribute('data-filter-type');
          const filterVal = pill.getAttribute('data-filter-val');
          if (!filterType || !filterVal) return;

          // Update active pill in this group
          pills.forEach(p => {{
            if (p.getAttribute('data-filter-type') === filterType) {{
              p.classList.remove('active');
            }}
          }});
          pill.classList.add('active');

          state[filterType] = filterVal;
          applyFilters();
        }});
      }});

      searchInput.addEventListener('input', (e) => {{
        state.search = e.target.value.trim().toLowerCase();
        applyFilters();
      }});

      function applyFilters() {{
        let visible = 0;
        cards.forEach(card => {{
          const cardSev = card.getAttribute('data-severity');
          const cardCat = card.getAttribute('data-category');
          const cardPol = card.getAttribute('data-policy');
          const cardSearch = card.getAttribute('data-search') || '';

          const matchSev = state.severity === 'all' || cardSev === state.severity;
          const matchCat = state.category === 'all' || cardCat === state.category;
          const matchPol = state.policy === 'all' || cardPol === state.policy || cardPol.startsWith(state.policy);
          const matchSearch = !state.search || cardSearch.includes(state.search);

          if (matchSev && matchCat && matchPol && matchSearch) {{
            card.style.display = '';
            visible++;
          }} else {{
            card.style.display = 'none';
          }}
        }});
        visibleCountEl.textContent = visible;
      }}
    }})();
  </script>
</body>
</html>
"#,
        run_id
    );

    html
}

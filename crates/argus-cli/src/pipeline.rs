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

//! Registry of the review pipelines the CLI can plan, execute, and report.
//!
//! Every command that branches on a pipeline matches on [`Pipeline`] so that adding a
//! pipeline is enforced by the compiler instead of by searching for string literals.

use std::collections::BTreeSet;

/// One policy review pipeline.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum Pipeline {
    Documentation,
    InternalDocumentation,
    Correctness,
    Architecture,
    Optimization,
    Maintainability,
    Conformance,
}

impl Pipeline {
    /// Every pipeline, in `full` admission and `work all` execution order.
    pub(crate) const ALL: [Self; 7] = [
        Self::Documentation,
        Self::InternalDocumentation,
        Self::Correctness,
        Self::Architecture,
        Self::Optimization,
        Self::Maintainability,
        Self::Conformance,
    ];

    /// Parses a user-supplied pipeline name, accepting `performance` as an alias.
    pub(crate) fn parse(name: &str) -> Option<Self> {
        match name {
            "performance" => Some(Self::Optimization),
            _ => Self::ALL
                .into_iter()
                .find(|pipeline| pipeline.name() == name),
        }
    }

    /// Canonical command-line name.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Documentation => "documentation",
            Self::InternalDocumentation => "internal-documentation",
            Self::Correctness => "correctness",
            Self::Architecture => "architecture",
            Self::Optimization => "optimization",
            Self::Maintainability => "maintainability",
            Self::Conformance => "conformance",
        }
    }

    /// Human-readable label used in status and summaries.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Documentation => "Documentation",
            Self::InternalDocumentation => "Internal documentation",
            Self::Correctness => "Correctness",
            Self::Architecture => "Architecture",
            Self::Optimization => "Optimization",
            Self::Maintainability => "Maintainability",
            Self::Conformance => "Conformance",
        }
    }

    /// Policy version recorded on admitted work items.
    pub(crate) const fn policy_version(self) -> &'static str {
        match self {
            Self::Documentation => "documentation-public-api@1",
            Self::InternalDocumentation => "documentation-internal@1",
            Self::Correctness => "correctness-conservative@1",
            Self::Architecture => "architecture-code-derived@1",
            Self::Optimization => "optimization-conservative@1",
            Self::Maintainability => "maintainability-conservative@1",
            Self::Conformance => "conformance-design-aligned@1",
        }
    }

    /// Whether a work item's coverage policy belongs to this pipeline.
    pub(crate) fn owns_policy(self, policy: &str) -> bool {
        match self {
            Self::Documentation | Self::InternalDocumentation => policy == self.policy_version(),
            _ => policy.starts_with(self.name()),
        }
    }

    /// Pipeline that owns a work item's coverage policy, if any.
    pub(crate) fn of_policy(policy: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|pipeline| pipeline.owns_policy(policy))
    }

    /// Pipelines with at least one admitted work item in a run.
    pub(crate) fn present_in(records: &argus_storage::RunRecords) -> BTreeSet<Self> {
        records
            .work
            .iter()
            .filter_map(|work| Self::of_policy(&work.coverage.policy))
            .collect()
    }
}

/// A policy report loaded for one pipeline.
pub(crate) struct PipelineReport {
    pub(crate) pipeline: Pipeline,
    report: Report,
}

enum Report {
    Documentation(argus_report::DocumentationReport),
    Correctness(argus_report::CorrectnessReport),
    Architecture(argus_report::ArchitectureReport),
    Optimization(argus_report::OptimizationReport),
    Maintainability(argus_report::MaintainabilityReport),
    Conformance(argus_report::ConformanceReport),
}

/// Evaluates `$body` with `$report` bound to whichever concrete report is loaded.
macro_rules! with_report {
    ($value:expr, $report:ident => $body:expr) => {
        match $value {
            Report::Documentation($report) => $body,
            Report::Correctness($report) => $body,
            Report::Architecture($report) => $body,
            Report::Optimization($report) => $body,
            Report::Maintainability($report) => $body,
            Report::Conformance($report) => $body,
        }
    };
}

/// Keeps finding clusters matching an optional dimension name and severity.
macro_rules! retain_clusters {
    ($report:expr, $dimension_type:ty, $label:literal, $dimension:expr, $severity:expr) => {{
        if let Some(name) = $dimension {
            let dimension: $dimension_type = serde_json::from_value(serde_json::Value::String(
                name.to_owned(),
            ))
            .map_err(|error| {
                argus_core::ArgusError::invalid_input(format!(
                    "unknown {} dimension `{name}`",
                    $label
                ))
                .with_source(error)
            })?;
            $report
                .finding_clusters
                .retain(|cluster| cluster.representative.dimensions.contains(&dimension));
        }
        if let Some(severity) = $severity {
            $report
                .finding_clusters
                .retain(|cluster| cluster.representative.severity == severity);
        }
    }};
}

impl PipelineReport {
    /// Builds the pipeline's report from the durable queue.
    pub(crate) fn load(
        pipeline: Pipeline,
        queue: &argus_storage::DurableQueue,
        run_id: &argus_core::RunId,
    ) -> Result<Self, argus_core::ArgusError> {
        let run_id = run_id.clone();
        let version = pipeline.policy_version();
        let report = match pipeline {
            Pipeline::Documentation | Pipeline::InternalDocumentation => Report::Documentation(
                argus_report::documentation_report_from_queue(queue, run_id, version)?,
            ),
            Pipeline::Correctness => Report::Correctness(
                argus_report::correctness_report_from_queue(queue, run_id, version)?,
            ),
            Pipeline::Architecture => Report::Architecture(
                argus_report::architecture_report_from_queue(queue, run_id, version)?,
            ),
            Pipeline::Optimization => Report::Optimization(
                argus_report::optimization_report_from_queue(queue, run_id, version)?,
            ),
            Pipeline::Maintainability => Report::Maintainability(
                argus_report::maintainability_report_from_queue(queue, run_id, version)?,
            ),
            Pipeline::Conformance => Report::Conformance(
                argus_report::conformance_report_from_queue(queue, run_id, version)?,
            ),
        };
        Ok(Self { pipeline, report })
    }

    /// Writes the pipeline's reports into a finalized run bundle.
    pub(crate) fn write_bundle(
        pipeline: Pipeline,
        destination: &std::path::Path,
        run_id: &argus_core::RunId,
    ) -> Result<Self, argus_core::ArgusError> {
        let run_id = run_id.clone();
        let version = pipeline.policy_version();
        let report = match pipeline {
            Pipeline::Documentation | Pipeline::InternalDocumentation => Report::Documentation(
                argus_report::write_documentation_bundle_reports(destination, run_id, version)?,
            ),
            Pipeline::Correctness => Report::Correctness(
                argus_report::write_correctness_bundle_reports(destination, run_id, version)?,
            ),
            Pipeline::Architecture => Report::Architecture(
                argus_report::write_architecture_bundle_reports(destination, run_id, version)?,
            ),
            Pipeline::Optimization => Report::Optimization(
                argus_report::write_optimization_bundle_reports(destination, run_id, version)?,
            ),
            Pipeline::Maintainability => Report::Maintainability(
                argus_report::write_maintainability_bundle_reports(destination, run_id, version)?,
            ),
            Pipeline::Conformance => Report::Conformance(
                argus_report::write_conformance_bundle_reports(destination, run_id, version)?,
            ),
        };
        Ok(Self { pipeline, report })
    }

    /// One-line summary used when a run is finalized.
    pub(crate) fn bundle_summary(&self) -> String {
        let name = self.pipeline.name().replace('-', " ");
        let (assessments, candidates, unadjudicated) = match &self.report {
            Report::Architecture(report) => (
                report.assessments.len(),
                report.summary.candidate_assessments,
                report.summary.unadjudicated_findings,
            ),
            Report::Documentation(report) => (
                report.assessments.len(),
                report.summary.candidate_findings,
                report.summary.unadjudicated_findings,
            ),
            Report::Correctness(report) => (
                report.assessments.len(),
                report.summary.candidate_findings,
                report.summary.unadjudicated_findings,
            ),
            Report::Optimization(report) => (
                report.assessments.len(),
                report.summary.candidate_findings,
                report.summary.unadjudicated_findings,
            ),
            Report::Maintainability(report) => (
                report.assessments.len(),
                report.summary.candidate_findings,
                report.summary.unadjudicated_findings,
            ),
            Report::Conformance(report) => (
                report.assessments.len(),
                report.summary.candidate_findings,
                report.summary.unadjudicated_findings,
            ),
        };
        if self.pipeline == Pipeline::InternalDocumentation {
            return format!("{assessments} {name} assessments");
        }
        format!(
            "{assessments} {name} assessments ({candidates} candidates, {unadjudicated} unadjudicated)"
        )
    }

    pub(crate) fn to_markdown(&self) -> String {
        with_report!(&self.report, report => report.to_markdown())
    }

    pub(crate) fn to_json_value(&self) -> Result<serde_json::Value, argus_core::ArgusError> {
        with_report!(&self.report, report => serde_json::to_value(report)).map_err(|error| {
            argus_core::ArgusError::invariant(format!(
                "cannot serialize {} report",
                self.pipeline.name()
            ))
            .with_source(error)
        })
    }

    pub(crate) fn to_json_pretty(&self) -> Result<String, argus_core::ArgusError> {
        with_report!(&self.report, report => serde_json::to_string_pretty(report)).map_err(
            |error| {
                argus_core::ArgusError::invariant(format!(
                    "cannot serialize {} report",
                    self.pipeline.name()
                ))
                .with_source(error)
            },
        )
    }

    /// Serializes each finding cluster as one JSON line.
    pub(crate) fn to_jsonl(&self) -> Result<String, argus_core::ArgusError> {
        let lines = with_report!(&self.report, report => report
            .finding_clusters
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>())
        .map_err(|error| {
            argus_core::ArgusError::invariant("cannot serialize finding cluster").with_source(error)
        })?;
        Ok(lines.join("\n"))
    }

    /// Finding clusters tagged with their pipeline, for mixed-policy JSON lines.
    pub(crate) fn tagged_findings(&self) -> Vec<serde_json::Value> {
        let policy = self.pipeline.name();
        with_report!(&self.report, report => report
            .finding_clusters
            .iter()
            .map(|finding| serde_json::json!({"policy": policy, "finding": finding}))
            .collect())
    }

    pub(crate) fn contains_finding(&self, finding: &argus_core::FindingId) -> bool {
        with_report!(&self.report, report => report
            .finding_clusters
            .iter()
            .any(|cluster| &cluster.id == finding))
    }

    /// Filters finding clusters by dimension name and severity.
    pub(crate) fn retain(
        &mut self,
        dimension: Option<&str>,
        severity: Option<argus_core::Severity>,
    ) -> Result<(), argus_core::ArgusError> {
        match &mut self.report {
            Report::Documentation(report) => retain_clusters!(
                report,
                argus_policies::DocumentationDimension,
                "documentation",
                dimension,
                severity
            ),
            Report::Correctness(report) => retain_clusters!(
                report,
                argus_policies::CorrectnessDimension,
                "correctness",
                dimension,
                severity
            ),
            Report::Architecture(report) => retain_clusters!(
                report,
                argus_policies::ArchitectureDimension,
                "architecture",
                dimension,
                severity
            ),
            Report::Optimization(report) => retain_clusters!(
                report,
                argus_policies::OptimizationDimension,
                "optimization",
                dimension,
                severity
            ),
            Report::Maintainability(report) => retain_clusters!(
                report,
                argus_policies::MaintainabilityDimension,
                "maintainability",
                dimension,
                severity
            ),
            Report::Conformance(report) => retain_clusters!(
                report,
                argus_policies::ConformanceDimension,
                "conformance",
                dimension,
                severity
            ),
        }
        Ok(())
    }

    /// Backlog items extracted from this report's findings.
    pub(crate) fn backlog_items(
        &self,
        run_id: &argus_core::RunId,
    ) -> Vec<argus_report::BacklogItem> {
        let run_id = run_id.clone();
        let backlog = match &self.report {
            Report::Documentation(report) => argus_report::extract_backlog_report(
                run_id,
                Some(report),
                None,
                None,
                None,
                None,
                None,
            ),
            Report::Correctness(report) => argus_report::extract_backlog_report(
                run_id,
                None,
                Some(report),
                None,
                None,
                None,
                None,
            ),
            Report::Architecture(report) => argus_report::extract_backlog_report(
                run_id,
                None,
                None,
                Some(report),
                None,
                None,
                None,
            ),
            Report::Optimization(report) => argus_report::extract_backlog_report(
                run_id,
                None,
                None,
                None,
                Some(report),
                None,
                None,
            ),
            Report::Maintainability(report) => argus_report::extract_backlog_report(
                run_id,
                None,
                None,
                None,
                None,
                Some(report),
                None,
            ),
            Report::Conformance(report) => argus_report::extract_backlog_report(
                run_id,
                None,
                None,
                None,
                None,
                None,
                Some(report),
            ),
        };
        let mut items = backlog.items;
        if self.pipeline == Pipeline::InternalDocumentation {
            for item in &mut items {
                item.policy = self.pipeline.name().to_owned();
            }
        }
        items
    }
}

#[cfg(test)]
mod tests {
    use super::Pipeline;

    #[test]
    fn names_round_trip_and_alias_resolves() {
        for pipeline in Pipeline::ALL {
            assert_eq!(Pipeline::parse(pipeline.name()), Some(pipeline));
        }
        assert_eq!(Pipeline::parse("performance"), Some(Pipeline::Optimization));
        assert_eq!(Pipeline::parse("full"), None);
        assert_eq!(Pipeline::parse("all"), None);
    }

    #[test]
    fn policy_versions_map_back_to_their_pipeline() {
        for pipeline in Pipeline::ALL {
            assert_eq!(
                Pipeline::of_policy(pipeline.policy_version()),
                Some(pipeline)
            );
        }
        assert_eq!(Pipeline::of_policy("unknown@1"), None);
    }

    #[test]
    fn help_text_lists_every_pipeline() {
        for pipeline in Pipeline::ALL {
            for (command, help) in [
                ("audit", crate::HELP_AUDIT),
                ("work", crate::HELP_WORK),
                ("evaluate", crate::HELP_EVALUATE),
            ] {
                assert!(
                    help.contains(pipeline.name()),
                    "`argus {command} --help` does not mention `{}`",
                    pipeline.name()
                );
            }
        }
    }
}

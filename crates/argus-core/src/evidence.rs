use crate::{ConfigurationId, EvidenceId, SourceLocation, TargetId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    Source,
    Documentation,
    Test,
    Benchmark,
    StaticAnalysis,
    CompilerDiagnostic,
    RuntimeMetric,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceOrigin {
    Direct,
    Inference,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionQuality {
    Exact,
    Inferred,
    ContainingTarget,
    FileFallback,
    Unmapped,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EvidenceProvenance {
    pub provider: String,
    pub provider_version: String,
    pub configuration: ConfigurationId,
    pub ingest_only: bool,
    pub resolution: ResolutionQuality,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EvidenceRecord {
    pub id: EvidenceId,
    pub kind: EvidenceKind,
    pub origin: EvidenceOrigin,
    pub target: Option<TargetId>,
    pub location: Option<SourceLocation>,
    pub summary: String,
    pub detail: Option<String>,
    pub provenance: EvidenceProvenance,
}

impl EvidenceRecord {
    pub fn validate(&self) -> Result<(), crate::ArgusError> {
        if self.summary.trim().is_empty()
            || self.provenance.provider.trim().is_empty()
            || self.provenance.provider_version.trim().is_empty()
        {
            return Err(crate::ArgusError::invariant(
                "evidence summary and provider identity are required",
            ));
        }
        if self.target.is_none() && self.provenance.resolution != ResolutionQuality::Unmapped {
            return Err(crate::ArgusError::invariant(
                "mapped evidence requires a target",
            ));
        }
        Ok(())
    }
}

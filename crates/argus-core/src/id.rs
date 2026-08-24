use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

const IDENTITY_VERSION: &[u8] = b"argus-id-v1";

macro_rules! define_id {
    ($name:ident, $namespace:literal) => {
        #[doc = concat!("Strong identifier in the `", $namespace, "` namespace.")]
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Derives a stable ID from an ordered sequence of immutable identity parts.
            #[must_use]
            pub fn derive<'a>(parts: impl IntoIterator<Item = &'a [u8]>) -> Self {
                let mut hasher = blake3::Hasher::new();
                add_part(&mut hasher, IDENTITY_VERSION);
                add_part(&mut hasher, $namespace.as_bytes());
                for part in parts {
                    add_part(&mut hasher, part);
                }
                Self(hasher.finalize().to_hex().to_string())
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = crate::ArgusError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    Ok(Self(value.to_ascii_lowercase()))
                } else {
                    Err(crate::ArgusError::invalid_input(concat!(
                        "invalid ",
                        $namespace,
                        " identifier"
                    )))
                }
            }
        }
    };
}

fn add_part(hasher: &mut blake3::Hasher, part: &[u8]) {
    hasher.update(&u64::try_from(part.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(part);
}

define_id!(SnapshotId, "snapshot");
define_id!(ConfigurationId, "configuration");
define_id!(TargetId, "target");
define_id!(RelationId, "relation");
define_id!(PolicyId, "policy");
define_id!(WorkItemId, "work-item");
define_id!(AttemptId, "attempt");
define_id!(AssessmentId, "assessment");
define_id!(FindingId, "finding");
define_id!(EvidenceId, "evidence");
define_id!(WorkflowId, "workflow");
define_id!(RunId, "run");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derivation_is_stable_and_namespaced() {
        let first = TargetId::derive([b"crate".as_slice(), b"item".as_slice()]);
        let repeat = TargetId::derive([b"crate".as_slice(), b"item".as_slice()]);
        let other_kind = PolicyId::derive([b"crate".as_slice(), b"item".as_slice()]);
        assert_eq!(first, repeat);
        assert_ne!(first.as_str(), other_kind.as_str());
    }

    #[test]
    fn length_prefix_prevents_ambiguous_tuples() {
        assert_ne!(
            TargetId::derive([b"ab".as_slice(), b"c".as_slice()]),
            TargetId::derive([b"a".as_slice(), b"bc".as_slice()])
        );
    }
}

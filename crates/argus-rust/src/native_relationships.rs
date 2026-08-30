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

use argus_core::{
    ConfigurationId, PortableTargetKind, Relation, RelationId, RelationProvenance,
    ResolutionQuality, SourcePath, Target, TargetId, TargetKind,
};
use argus_language::SourceAccess;
use ra_ap_syntax::{AstNode, Edition, SourceFile, SyntaxKind};
use std::collections::{BTreeMap, BTreeSet};

const PROVIDER: &str = "ra_ap_syntax-native-relations";
const PROVIDER_VERSION: &str = "1";

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NativeRustRelationshipInventory {
    pub relations: Vec<Relation>,
    pub ambiguous_names: BTreeSet<String>,
}

pub struct NativeRustRelationshipProvider {
    configuration: ConfigurationId,
}

impl NativeRustRelationshipProvider {
    #[must_use]
    pub const fn new(configuration: ConfigurationId) -> Self {
        Self { configuration }
    }

    pub fn infer(
        &self,
        source: &dyn SourceAccess,
        targets: &[Target],
    ) -> Result<NativeRustRelationshipInventory, argus_core::ArgusError> {
        let mut names: BTreeMap<&str, Vec<&Target>> = BTreeMap::new();
        for target in targets
            .iter()
            .filter(|target| is_reference_destination(target))
        {
            if is_identifier(&target.name) {
                names.entry(&target.name).or_default().push(target);
            }
        }
        let ambiguous_names = names
            .iter()
            .filter(|(_, targets)| targets.len() > 1)
            .map(|(name, _)| (*name).to_owned())
            .collect();
        let unique = names
            .into_iter()
            .filter_map(|(name, targets)| (targets.len() == 1).then_some((name, targets[0])))
            .collect::<BTreeMap<_, _>>();
        let mut sources = BTreeMap::<SourcePath, Vec<u8>>::new();
        let mut relations = BTreeMap::new();

        for owner in targets
            .iter()
            .filter(|target| is_relationship_source(target))
        {
            let Some(location) = &owner.location else {
                continue;
            };
            if !sources.contains_key(&location.path) {
                sources.insert(location.path.clone(), source.read(&location.path)?);
            }
            let bytes = sources
                .get(&location.path)
                .expect("relationship source was inserted");
            let start = usize::try_from(location.bytes.start).map_err(|error| {
                argus_core::ArgusError::invariant(
                    "native relationship source start exceeds platform limits",
                )
                .with_source(error)
            })?;
            let end = usize::try_from(location.bytes.end).map_err(|error| {
                argus_core::ArgusError::invariant(
                    "native relationship source end exceeds platform limits",
                )
                .with_source(error)
            })?;
            let fragment = std::str::from_utf8(bytes.get(start..end).ok_or_else(|| {
                argus_core::ArgusError::invariant(
                    "native relationship source span is out of bounds",
                )
            })?)
            .map_err(|error| {
                argus_core::ArgusError::invalid_input("native relationship source must be UTF-8")
                    .with_source(error)
            })?;

            for identifier in identifiers(fragment) {
                let Some(destination) = unique.get(identifier.name.as_str()) else {
                    continue;
                };
                if destination.id == owner.id {
                    continue;
                }
                insert_relation(
                    &mut relations,
                    owner.id.clone(),
                    destination.id.clone(),
                    "rust:references",
                    &self.configuration,
                    Some(format!("unique lexical reference `{}`", identifier.name)),
                );
                if identifier.followed_by_call && is_callable(destination) {
                    insert_relation(
                        &mut relations,
                        owner.id.clone(),
                        destination.id.clone(),
                        "rust:calls",
                        &self.configuration,
                        Some(format!("unique lexical call `{}`", identifier.name)),
                    );
                }
            }

            if is_impl(owner)
                && let Some((implementor, implemented_trait)) = impl_sides(fragment, &unique)
            {
                insert_relation(
                    &mut relations,
                    implementor.id.clone(),
                    implemented_trait.id.clone(),
                    "rust:implements",
                    &self.configuration,
                    Some(format!(
                        "unique impl header `{} for {}`",
                        implemented_trait.name, implementor.name
                    )),
                );
            }
        }

        Ok(NativeRustRelationshipInventory {
            relations: relations.into_values().collect(),
            ambiguous_names,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Identifier {
    name: String,
    followed_by_call: bool,
}

fn identifiers(source: &str) -> Vec<Identifier> {
    let parsed = SourceFile::parse(source, Edition::Edition2024).tree();
    let tokens = parsed
        .syntax()
        .descendants_with_tokens()
        .filter_map(|element| element.into_token())
        .collect::<Vec<_>>();
    tokens
        .iter()
        .enumerate()
        .filter(|(_, token)| token.kind() == SyntaxKind::IDENT && is_identifier(token.text()))
        .map(|(index, token)| Identifier {
            name: token.text().to_string(),
            followed_by_call: tokens
                .iter()
                .skip(index + 1)
                .find(|next| !next.kind().is_trivia())
                .is_some_and(|next| next.kind() == SyntaxKind::L_PAREN),
        })
        .collect()
}

fn impl_sides<'a>(
    source: &str,
    unique: &BTreeMap<&str, &'a Target>,
) -> Option<(&'a Target, &'a Target)> {
    let header = source.split_once('{').map_or(source, |(header, _)| header);
    let (trait_side, implementor_side) = header.split_once(" for ")?;
    let implemented_trait = identifiers(trait_side)
        .into_iter()
        .rev()
        .find_map(|identifier| unique.get(identifier.name.as_str()).copied())?;
    let implementor = identifiers(implementor_side)
        .into_iter()
        .find_map(|identifier| unique.get(identifier.name.as_str()).copied())?;
    (implementor.id != implemented_trait.id).then_some((implementor, implemented_trait))
}

fn insert_relation(
    relations: &mut BTreeMap<RelationId, Relation>,
    source: TargetId,
    target: TargetId,
    kind: &str,
    configuration: &ConfigurationId,
    detail: Option<String>,
) {
    let id = RelationId::derive([
        source.as_str().as_bytes(),
        target.as_str().as_bytes(),
        kind.as_bytes(),
        PROVIDER.as_bytes(),
        PROVIDER_VERSION.as_bytes(),
        configuration.as_str().as_bytes(),
    ]);
    relations.entry(id.clone()).or_insert(Relation {
        id,
        source,
        target,
        kind: kind.to_owned(),
        provenance: RelationProvenance {
            provider: PROVIDER.to_owned(),
            provider_version: PROVIDER_VERSION.to_owned(),
            configuration: Some(configuration.clone()),
            ingest_only: true,
            resolution: ResolutionQuality::Inferred,
            detail,
        },
    });
}

fn is_identifier(value: &str) -> bool {
    let mut characters = value.chars();
    characters
        .next()
        .is_some_and(|character| character == '_' || character.is_alphabetic())
        && characters.all(|character| character == '_' || character.is_alphanumeric())
}

fn is_reference_destination(target: &Target) -> bool {
    match &target.kind {
        TargetKind::Portable { kind } => matches!(
            kind,
            PortableTargetKind::Module
                | PortableTargetKind::Type
                | PortableTargetKind::Callable
                | PortableTargetKind::Constant
                | PortableTargetKind::Test
        ),
        TargetKind::LanguageSpecific { language, kind } => {
            language == "rust"
                && matches!(
                    kind.as_str(),
                    "method" | "trait_method" | "type_alias" | "associated_type"
                )
        }
    }
}

fn is_relationship_source(target: &Target) -> bool {
    match &target.kind {
        TargetKind::Portable { kind } => matches!(
            kind,
            PortableTargetKind::Type
                | PortableTargetKind::Callable
                | PortableTargetKind::Constant
                | PortableTargetKind::Test
        ),
        TargetKind::LanguageSpecific { language, kind } => {
            language == "rust"
                && matches!(
                    kind.as_str(),
                    "impl" | "method" | "trait_method" | "type_alias" | "associated_type"
                )
        }
    }
}

fn is_callable(target: &Target) -> bool {
    matches!(
        target.kind,
        TargetKind::Portable {
            kind: PortableTargetKind::Callable | PortableTargetKind::Test
        }
    ) || matches!(
        &target.kind,
        TargetKind::LanguageSpecific { language, kind }
            if language == "rust" && matches!(kind.as_str(), "method" | "trait_method")
    )
}

fn is_impl(target: &Target) -> bool {
    matches!(
        &target.kind,
        TargetKind::LanguageSpecific { language, kind }
            if language == "rust" && kind == "impl"
    )
}

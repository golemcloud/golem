// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::value::WorkBudget;
use super::{ToolIdentifier, ToolPatternError, ToolValuePattern};
use serde::{Deserialize, Serialize};

/// Primary long names only. Occurrences are retained, including repeated count flags.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct CanonicalToolFlag<V> {
    pub name: ToolIdentifier,
    pub value: Option<V>,
}

/// An empty command path addresses the root command, not all subcommands.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct CanonicalToolInvocation<V: 'static> {
    pub command_path: Vec<ToolIdentifier>,
    pub flags: Vec<CanonicalToolFlag<V>>,
    pub positionals: Vec<V>,
}

pub type CanonicalToolPattern = CanonicalToolInvocation<ToolValuePattern>;
pub type ToolInvocation = CanonicalToolInvocation<ToolInvocationValue>;

/// Admission never reads a stream to discover its contents. Opaque values can be any string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ToolInvocationValue {
    Literal(String),
    Opaque,
}

impl CanonicalToolPattern {
    pub fn subsumes_invocation_pattern(&self, child: &Self) -> Result<bool, ToolPatternError> {
        self.covers(child, |parent, child, budget| {
            parent.subsumes_with_budget(child, budget)
        })
    }

    /// Every possible completion of opaque values must satisfy the positive constraint.
    pub fn definitely_matches(
        &self,
        invocation: &ToolInvocation,
    ) -> Result<bool, ToolPatternError> {
        self.covers(invocation, |pattern, value, budget| match value {
            ToolInvocationValue::Literal(value) => pattern.matches_with_budget(value, budget),
            ToolInvocationValue::Opaque => {
                pattern.subsumes_with_budget(&ToolValuePattern::GlobStar, budget)
            }
        })
    }

    /// A denial applies if some completion of the opaque values satisfies it.
    pub fn possibly_matches(&self, invocation: &ToolInvocation) -> Result<bool, ToolPatternError> {
        self.covers(invocation, |pattern, value, budget| match value {
            ToolInvocationValue::Literal(value) => pattern.matches_with_budget(value, budget),
            ToolInvocationValue::Opaque => {
                pattern.overlaps_with_budget(&ToolValuePattern::GlobStar, budget)
            }
        })
    }

    fn covers<V: 'static>(
        &self,
        child: &CanonicalToolInvocation<V>,
        value_covers: impl Fn(&ToolValuePattern, &V, &mut WorkBudget) -> Result<bool, ToolPatternError>,
    ) -> Result<bool, ToolPatternError> {
        if self.command_path != child.command_path
            || self.positionals.len() > child.positionals.len()
            || self.flags.len() > child.flags.len()
        {
            return Ok(false);
        }
        // Bounds both the occurrence graph and augmenting-path stack depth.
        if self.flags.len() > 128 || child.flags.len() > 128 || self.positionals.len() > 128 {
            return Err(ToolPatternError::ComplexityLimit);
        }
        let mut budget = WorkBudget::new();
        for (parent, child) in self.positionals.iter().zip(&child.positionals) {
            if !value_covers(parent, child, &mut budget)? {
                return Ok(false);
            }
        }
        let mut edges = Vec::with_capacity(self.flags.len());
        for required in &self.flags {
            let mut candidates = Vec::new();
            for (index, offered) in child.flags.iter().enumerate() {
                if required.name == offered.name
                    && match (&required.value, &offered.value) {
                        (None, _) => true,
                        (Some(parent), Some(child)) => value_covers(parent, child, &mut budget)?,
                        (Some(_), None) => false,
                    }
                {
                    candidates.push(index);
                }
            }
            edges.push(candidates);
        }
        let mut assigned = vec![None; child.flags.len()];
        for requirement in 0..edges.len() {
            if !assign(
                requirement,
                &edges,
                &mut assigned,
                &mut vec![false; child.flags.len()],
            ) {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

fn assign(
    requirement: usize,
    edges: &[Vec<usize>],
    assigned: &mut [Option<usize>],
    visited: &mut [bool],
) -> bool {
    for &candidate in &edges[requirement] {
        if visited[candidate] {
            continue;
        }
        visited[candidate] = true;
        if assigned[candidate].is_none_or(|previous| assign(previous, edges, assigned, visited)) {
            assigned[candidate] = Some(requirement);
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn pattern(flags: &[(&str, Option<&str>)], positionals: &[&str]) -> CanonicalToolPattern {
        CanonicalToolPattern {
            command_path: vec![ToolIdentifier("status".into())],
            flags: flags
                .iter()
                .map(|(name, value)| CanonicalToolFlag {
                    name: ToolIdentifier((*name).into()),
                    value: value.map(|v| ToolValuePattern::parse_glob(v).unwrap()),
                })
                .collect(),
            positionals: positionals
                .iter()
                .map(|v| ToolValuePattern::parse_glob(v).unwrap())
                .collect(),
        }
    }

    #[test]
    fn structural_requirements_preserve_order_and_multiplicity() {
        let parent = pattern(
            &[("include", Some("*")), ("include", Some("a"))],
            &["source", "target"],
        );
        let child = pattern(
            &[
                ("include", Some("a")),
                ("extra", None),
                ("include", Some("b")),
            ],
            &["source", "target", "trailing"],
        );
        assert!(parent.subsumes_invocation_pattern(&child).unwrap());
        assert!(!child.subsumes_invocation_pattern(&parent).unwrap());
        assert!(
            !parent
                .subsumes_invocation_pattern(&pattern(
                    &[("include", Some("a"))],
                    &["source", "target"]
                ))
                .unwrap()
        );
        assert!(
            !parent
                .subsumes_invocation_pattern(&pattern(
                    &[("include", Some("a")), ("include", Some("b"))],
                    &["target", "source"]
                ))
                .unwrap()
        );
        let two = pattern(&[("verbose", None), ("verbose", None)], &[]);
        assert!(
            !two.subsumes_invocation_pattern(&pattern(&[("verbose", None)], &[]))
                .unwrap()
        );
        assert!(
            pattern(&[], &[])
                .subsumes_invocation_pattern(&child)
                .unwrap()
        );
        let mut root = pattern(&[], &[]);
        root.command_path.clear();
        assert!(!root.subsumes_invocation_pattern(&child).unwrap());
    }

    #[test]
    fn opaque_values_require_definite_grants_and_possible_denials() {
        let invocation = ToolInvocation {
            command_path: vec![ToolIdentifier("status".into())],
            flags: vec![CanonicalToolFlag {
                name: ToolIdentifier("cwd".into()),
                value: Some(ToolInvocationValue::Opaque),
            }],
            positionals: vec![],
        };
        let restricted = pattern(&[("cwd", Some("/workspace/**"))], &[]);
        assert!(!restricted.definitely_matches(&invocation).unwrap());
        assert!(restricted.possibly_matches(&invocation).unwrap());
        assert!(
            !pattern(&[("cwd", Some("*"))], &[])
                .definitely_matches(&invocation)
                .unwrap()
        );
        assert!(
            pattern(&[("cwd", Some("**"))], &[])
                .definitely_matches(&invocation)
                .unwrap()
        );
        assert!(pattern(&[], &[]).definitely_matches(&invocation).unwrap());
        assert!(
            !pattern(&[("absent", None)], &[])
                .possibly_matches(&invocation)
                .unwrap()
        );
    }

    #[test]
    fn repeated_constraints_share_one_work_budget() {
        let value = "x".repeat(400);
        let pattern = pattern(&vec![("include", Some(value.as_str())); 128], &[]);
        assert_eq!(
            pattern.subsumes_invocation_pattern(&pattern),
            Err(ToolPatternError::ComplexityLimit)
        );
    }
}

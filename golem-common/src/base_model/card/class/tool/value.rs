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

use super::{ToolValueLiteral, ToolValuePattern};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet, VecDeque};

const MAX_TOKENS: usize = 512;
const MAX_STATES: usize = 8192;
const MAX_WORK: usize = 1_000_000;

pub(super) struct WorkBudget(usize);

impl WorkBudget {
    pub(super) fn new() -> Self {
        Self(MAX_WORK)
    }

    fn charge(&mut self, work: usize) -> Result<(), ToolPatternError> {
        self.0 = self
            .0
            .checked_sub(work)
            .ok_or(ToolPatternError::ComplexityLimit)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ToolGlobToken {
    Literal(char),
    Star,
    GlobStar,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ToolPatternError {
    #[error("tool permission pattern exceeds the matching complexity limit")]
    ComplexityLimit,
    #[error("tool permission value ends with an incomplete escape")]
    IncompleteEscape,
    #[error("tool permission requires a pinned command definition: {0}")]
    MissingDefinition(String),
    #[error("invalid tool permission: {0}")]
    Invalid(String),
}

impl ToolValuePattern {
    /// Backslash quotes the next character; all other non-star characters are literal.
    pub fn parse_glob(value: &str) -> Result<Self, ToolPatternError> {
        let mut chars = value.chars().peekable();
        let mut tokens = Vec::new();
        while let Some(c) = chars.next() {
            tokens.push(match c {
                '\\' => {
                    ToolGlobToken::Literal(chars.next().ok_or(ToolPatternError::IncompleteEscape)?)
                }
                '*' if chars.peek() == Some(&'*') => {
                    chars.next();
                    ToolGlobToken::GlobStar
                }
                '*' => ToolGlobToken::Star,
                c => ToolGlobToken::Literal(c),
            });
            if tokens.len() > MAX_TOKENS {
                return Err(ToolPatternError::ComplexityLimit);
            }
        }
        Ok(match tokens.as_slice() {
            [ToolGlobToken::Star] => Self::Star,
            [ToolGlobToken::GlobStar] => Self::GlobStar,
            _ if tokens
                .iter()
                .all(|t| matches!(t, ToolGlobToken::Literal(_))) =>
            {
                Self::Literal(ToolValueLiteral(
                    tokens
                        .iter()
                        .map(|t| match t {
                            ToolGlobToken::Literal(c) => *c,
                            _ => unreachable!(),
                        })
                        .collect(),
                ))
            }
            _ => Self::Glob(tokens),
        })
    }

    pub fn render_glob(&self) -> String {
        let mut tokens = self.tokens_unchecked().into_iter().peekable();
        let mut result = String::new();
        while let Some(token) = tokens.next() {
            match token {
                ToolGlobToken::Literal(c) => {
                    if c == '*' || c == '\\' || c.is_whitespace() || c == '"' || c == '\'' {
                        result.push('\\');
                    }
                    result.push(c);
                }
                ToolGlobToken::Star | ToolGlobToken::GlobStar => {
                    let mut crosses_slash = token == ToolGlobToken::GlobStar;
                    while matches!(
                        tokens.peek(),
                        Some(ToolGlobToken::Star | ToolGlobToken::GlobStar)
                    ) {
                        crosses_slash |= tokens.next() == Some(ToolGlobToken::GlobStar);
                    }
                    result.push_str(if crosses_slash { "**" } else { "*" });
                }
            }
        }
        result
    }

    pub fn matches_value(&self, value: &str) -> Result<bool, ToolPatternError> {
        self.matches_with_budget(value, &mut WorkBudget::new())
    }

    pub(super) fn split_on_literal(&self, separator: char) -> Vec<Self> {
        let tokens = self.tokens_unchecked();
        tokens
            .split(|t| *t == ToolGlobToken::Literal(separator))
            .map(|part| match part {
                [ToolGlobToken::Star] => Self::Star,
                [ToolGlobToken::GlobStar] => Self::GlobStar,
                _ if part.iter().all(|t| matches!(t, ToolGlobToken::Literal(_))) => {
                    Self::Literal(ToolValueLiteral(
                        part.iter()
                            .filter_map(|t| match t {
                                ToolGlobToken::Literal(c) => Some(*c),
                                _ => None,
                            })
                            .collect(),
                    ))
                }
                _ => Self::Glob(part.to_vec()),
            })
            .collect()
    }

    pub(super) fn matches_with_budget(
        &self,
        value: &str,
        budget: &mut WorkBudget,
    ) -> Result<bool, ToolPatternError> {
        let tokens = self.tokens()?;
        budget.charge(tokens.len() + 1)?;
        let mut states = closure(&tokens, vec![0]);
        for c in value.chars() {
            budget.charge(tokens.len() + 1)?;
            states = advance(&tokens, &states, Some(c));
            if states.is_empty() {
                return Ok(false);
            }
        }
        Ok(states.contains(&tokens.len()))
    }

    pub fn subsumes_value(&self, child: &Self) -> Result<bool, ToolPatternError> {
        self.subsumes_with_budget(child, &mut WorkBudget::new())
    }

    pub(super) fn subsumes_with_budget(
        &self,
        child: &Self,
        budget: &mut WorkBudget,
    ) -> Result<bool, ToolPatternError> {
        let parent = self.tokens()?;
        let child = child.tokens()?;
        Ok(!product_witness(&parent, &child, budget, |p, c| c && !p)?)
    }

    pub fn overlaps_value(&self, other: &Self) -> Result<bool, ToolPatternError> {
        self.overlaps_with_budget(other, &mut WorkBudget::new())
    }

    pub(super) fn overlaps_with_budget(
        &self,
        other: &Self,
        budget: &mut WorkBudget,
    ) -> Result<bool, ToolPatternError> {
        product_witness(&self.tokens()?, &other.tokens()?, budget, |a, b| a && b)
    }

    fn tokens_unchecked(&self) -> Vec<ToolGlobToken> {
        match self {
            Self::Literal(ToolValueLiteral(value)) => {
                value.chars().map(ToolGlobToken::Literal).collect()
            }
            Self::Star => vec![ToolGlobToken::Star],
            Self::GlobStar => vec![ToolGlobToken::GlobStar],
            Self::Glob(tokens) => tokens.clone(),
        }
    }

    fn tokens(&self) -> Result<Vec<ToolGlobToken>, ToolPatternError> {
        let tokens = self.tokens_unchecked();
        if tokens.len() > MAX_TOKENS {
            Err(ToolPatternError::ComplexityLimit)
        } else {
            Ok(tokens)
        }
    }
}

fn closure(tokens: &[ToolGlobToken], states: Vec<usize>) -> Vec<usize> {
    let mut present = vec![false; tokens.len() + 1];
    for state in states {
        present[state] = true;
    }
    for (index, token) in tokens.iter().enumerate() {
        if present[index] && matches!(token, ToolGlobToken::Star | ToolGlobToken::GlobStar) {
            present[index + 1] = true;
        }
    }
    present
        .into_iter()
        .enumerate()
        .filter_map(|(i, yes)| yes.then_some(i))
        .collect()
}

// None represents every character outside the finite literal alphabet, excluding slash.
fn advance(tokens: &[ToolGlobToken], states: &[usize], c: Option<char>) -> Vec<usize> {
    let mut next = Vec::new();
    for &state in states {
        match tokens.get(state) {
            Some(ToolGlobToken::Literal(literal)) if Some(*literal) == c => next.push(state + 1),
            Some(ToolGlobToken::Star) if c != Some('/') => next.push(state),
            Some(ToolGlobToken::GlobStar) => next.push(state),
            _ => {}
        }
    }
    closure(tokens, next)
}

fn product_witness(
    a: &[ToolGlobToken],
    b: &[ToolGlobToken],
    budget: &mut WorkBudget,
    accepts: impl Fn(bool, bool) -> bool,
) -> Result<bool, ToolPatternError> {
    budget.charge(a.len() + b.len() + 2)?;
    let mut alphabet = BTreeSet::from([Some('/'), None]);
    for token in a.iter().chain(b) {
        if let ToolGlobToken::Literal(c) = token {
            alphabet.insert(Some(*c));
        }
    }
    let initial = (closure(a, vec![0]), closure(b, vec![0]));
    let mut seen = HashSet::from([initial.clone()]);
    let mut pending = VecDeque::from([initial]);
    while let Some((left, right)) = pending.pop_front() {
        if accepts(left.contains(&a.len()), right.contains(&b.len())) {
            return Ok(true);
        }
        for &c in &alphabet {
            budget.charge(a.len() + b.len() + 2)?;
            let next = (advance(a, &left, c), advance(b, &right, c));
            if seen.insert(next.clone()) {
                if seen.len() > MAX_STATES {
                    return Err(ToolPatternError::ComplexityLimit);
                }
                pending.push_back(next);
            }
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use test_r::test;

    #[test]
    fn anchored_globs_and_literal_boundaries() {
        for (pattern, value, expected) in [
            ("*", "", true),
            ("**", "", true),
            ("/workspace/*", "/workspace/", true),
            ("/workspace/*", "/workspace/a", true),
            ("/workspace/*", "/workspace/a/b", false),
            ("/workspace/**", "/workspace/a/b", true),
            ("/workspace/**", "/workspace", false),
            ("/workspace/**", "/private/a", false),
            ("a/**/b", "a/b", false),
            ("a/**/b", "a//b", true),
            ("\\*", "*", true),
            ("\\*", "other", false),
            ("α*β", "α/β", false),
            ("α**β", "α/β", true),
        ] {
            let pattern = ToolValuePattern::parse_glob(pattern).unwrap();
            assert_eq!(
                pattern.matches_value(value).unwrap(),
                expected,
                "{pattern:?} {value}"
            );
            assert_eq!(
                ToolValuePattern::parse_glob(&pattern.render_glob()).unwrap(),
                pattern
            );
        }
        let wide = ToolValuePattern::parse_glob("/workspace/**").unwrap();
        let narrow = ToolValuePattern::parse_glob("/workspace/*").unwrap();
        assert!(wide.subsumes_value(&narrow).unwrap());
        assert!(!narrow.subsumes_value(&wide).unwrap());
        assert!(
            !wide
                .overlaps_value(&ToolValuePattern::parse_glob("/private/**").unwrap())
                .unwrap()
        );
        assert!(ToolValuePattern::parse_glob(&"x".repeat(MAX_TOKENS + 1)).is_err());
    }

    #[test]
    fn rendering_adjacent_wildcards_preserves_the_language() {
        for tokens in [
            vec![ToolGlobToken::Star, ToolGlobToken::Star],
            vec![
                ToolGlobToken::Star,
                ToolGlobToken::GlobStar,
                ToolGlobToken::Star,
            ],
        ] {
            let pattern = ToolValuePattern::Glob(tokens);
            let rendered = ToolValuePattern::parse_glob(&pattern.render_glob()).unwrap();
            for witness in ["", "/", "ab", "a/b"] {
                assert_eq!(
                    pattern.matches_value(witness).unwrap(),
                    rendered.matches_value(witness).unwrap()
                );
            }
        }
        assert_eq!(
            ToolValuePattern::GlobStar.matches_value(&"x".repeat(MAX_WORK)),
            Err(ToolPatternError::ComplexityLimit)
        );
    }

    // Independent recursive reference, only used for short generated witnesses.
    fn reference(pattern: &[ToolGlobToken], value: &[char]) -> bool {
        match pattern.split_first() {
            None => value.is_empty(),
            Some((ToolGlobToken::Literal(c), rest)) => {
                value.first() == Some(c) && reference(rest, &value[1..])
            }
            Some((wildcard, rest)) => (0..=value.len()).any(|n| {
                (*wildcard == ToolGlobToken::GlobStar || !value[..n].contains(&'/'))
                    && reference(rest, &value[n..])
            }),
        }
    }

    #[test]
    fn glob_inclusion_is_sound_for_independent_witnesses() {
        proptest!(|(parent in "[ab/*]{0,8}", child in "[ab/*]{0,8}", value in "[ab/]{0,8}")| {
            let parent = ToolValuePattern::parse_glob(&parent).unwrap();
            let child = ToolValuePattern::parse_glob(&child).unwrap();
            let chars = value.chars().collect::<Vec<_>>();
            let pm = reference(&parent.tokens().unwrap(), &chars);
            let cm = reference(&child.tokens().unwrap(), &chars);
            prop_assert_eq!(parent.matches_value(&value).unwrap(), pm);
            if parent.subsumes_value(&child).unwrap() && cm {
                prop_assert!(pm);
            }
            if pm && cm {
                prop_assert!(parent.overlaps_value(&child).unwrap());
            }
        });
    }
}

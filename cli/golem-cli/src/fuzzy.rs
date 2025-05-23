// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use colored::Colorize;
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use itertools::{Either, Itertools};
use std::collections::HashSet;
use std::fmt::Write;

pub struct Match {
    pub option: String,
    pub pattern: String,
    pub exact_match: bool,
}

#[derive(Debug)]
pub enum Error {
    Ambiguous {
        pattern: String,
        highlighted_options: Vec<String>,
        raw_options: Vec<String>,
    },
    NotFound {
        pattern: String,
    },
}

pub type Result = std::result::Result<Match, Error>;

pub struct FuzzySearch<'a> {
    options: HashSet<&'a str>,
    matcher: SkimMatcherV2,
}

impl<'a> FuzzySearch<'a> {
    pub fn new<I: Iterator<Item = &'a str>>(options: I) -> Self {
        let options_set = HashSet::from_iter(options);
        Self {
            options: options_set,
            matcher: SkimMatcherV2::default(),
        }
    }

    pub fn find(&self, pattern: &str) -> Result {
        // Exact matches
        if let Some(option) = self.options.get(pattern) {
            return Ok(Match {
                option: option.to_string(),
                pattern: pattern.to_string(),
                exact_match: true,
            });
        }

        // Contains matches
        let contains_matches = self
            .options
            .iter()
            .filter(|&option| option.contains(pattern))
            .collect::<Vec<_>>();

        if contains_matches.len() == 1 {
            return Ok(Match {
                option: contains_matches[0].to_string(),
                pattern: pattern.to_string(),
                exact_match: false,
            });
        }

        // Fuzzy matches
        let fuzzy_matches = self
            .options
            .iter()
            .filter_map(|option| {
                self.matcher
                    .fuzzy_indices(option, pattern)
                    .map(|(score, indices)| (score, indices, option))
            })
            .sorted_by(|(score_a, _, _), (score_b, _, _)| Ord::cmp(score_b, score_a))
            .collect::<Vec<_>>();

        match fuzzy_matches.len() {
            0 => Err(Error::NotFound {
                pattern: pattern.to_string(),
            }),
            1 => Ok(Match {
                option: fuzzy_matches[0].2.to_string(),
                pattern: pattern.to_string(),
                exact_match: false,
            }),
            _ => Err(Error::Ambiguous {
                pattern: pattern.to_string(),
                raw_options: fuzzy_matches
                    .iter()
                    .map(|(_, _, option)| option.to_string())
                    .collect(),
                highlighted_options: fuzzy_matches
                    .into_iter()
                    .map(|(_, indices, option)| {
                        let indices = HashSet::<usize>::from_iter(indices);
                        let mut highlighted_option = String::with_capacity(option.len() * 2);
                        for (idx, char) in option.chars().enumerate() {
                            if indices.contains(&idx) {
                                highlighted_option
                                    .write_fmt(format_args!(
                                        "{}",
                                        char.to_string().green().underline()
                                    ))
                                    .unwrap();
                            } else {
                                highlighted_option.write_char(char).unwrap()
                            }
                        }
                        highlighted_option
                    })
                    .collect(),
            }),
        }
    }

    pub fn find_many<I: Iterator<Item = &'a str>>(&self, patterns: I) -> (Vec<Match>, Vec<Error>) {
        patterns
            .map(|pattern| self.find(pattern))
            .partition_map(|result| match result {
                Ok(match_) => Either::Left(match_),
                Err(error) => Either::Right(error),
            })
    }
}

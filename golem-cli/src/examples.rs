// Copyright 2024 Golem Cloud
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

use std::env;

use golem_examples::model::{
    ExampleName, ExampleParameters, GuestLanguage, GuestLanguageTier, PackageName, TemplateName,
};
use golem_examples::*;

use crate::model::{ExampleDescription, GolemError, GolemResult};

pub fn process_new(
    example_name: ExampleName,
    template_name: TemplateName,
    package_name: Option<PackageName>,
) -> Result<GolemResult, GolemError> {
    let examples = GolemExamples::list_all_examples();
    let example = examples.iter().find(|example| example.name == example_name);
    match example {
        Some(example) => {
            let cwd = env::current_dir().expect("Failed to get current working directory");
            match GolemExamples::instantiate(
                example,
                ExampleParameters {
                    template_name,
                    package_name: package_name
                        .unwrap_or(PackageName::from_string("golem:template").unwrap()),
                    target_path: cwd,
                },
            ) {
                Ok(instructions) => Ok(GolemResult::Str(instructions.to_string())),
                Err(err) => GolemResult::err(format!("Failed to instantiate template: {err}")),
            }
        }
        None => {
            GolemResult::err(format!("Unknown template {example_name}. Use the list-templates command to see the available commands."))
        }
    }
}

pub fn process_list_examples(
    min_tier: Option<GuestLanguageTier>,
    language: Option<GuestLanguage>,
) -> Result<GolemResult, GolemError> {
    let examples = GolemExamples::list_all_examples()
        .iter()
        .filter(|example| match &language {
            Some(language) => example.language == *language,
            None => true,
        })
        .filter(|example| match &min_tier {
            Some(min_tier) => example.language.tier() <= *min_tier,
            None => true,
        })
        .map(ExampleDescription::from_example)
        .collect::<Vec<ExampleDescription>>();

    Ok(GolemResult::Ok(Box::new(examples)))
}

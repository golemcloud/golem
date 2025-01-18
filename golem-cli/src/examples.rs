// Copyright 2024-2025 Golem Cloud
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

use crate::model::{ExampleDescription, GolemError, GolemResult};
use golem_examples::model::{
    ComponentName, ComposableAppGroupName, ExampleName, ExampleParameters, GuestLanguage,
    GuestLanguageTier, PackageName, TargetExistsResolveMode,
};
use golem_examples::*;
use itertools::Itertools;

pub fn new(
    example_name: ExampleName,
    component_name: ComponentName,
    package_name: Option<PackageName>,
) -> Result<GolemResult, GolemError> {
    let examples = all_standalone_examples();
    let example = examples.iter().find(|example| example.name == example_name);
    match example {
        Some(example) => {
            let cwd = env::current_dir().expect("Failed to get current working directory");
            match instantiate_example(
                example,
                &ExampleParameters {
                    component_name,
                    package_name: package_name
                        .unwrap_or(PackageName::from_string("golem:component").unwrap()),
                    target_path: cwd,
                },
                TargetExistsResolveMode::Fail,
            ) {
                Ok(instructions) => Ok(GolemResult::Str(instructions.to_string())),
                Err(err) => GolemResult::err(format!("Failed to instantiate component: {err}")),
            }
        }
        None => {
            GolemResult::err(format!("Unknown component {example_name}. Use the list-examples command to see the available examples."))
        }
    }
}

pub fn list_standalone_examples(
    min_tier: Option<GuestLanguageTier>,
    language: Option<GuestLanguage>,
) -> Result<GolemResult, GolemError> {
    let examples = all_standalone_examples()
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

pub fn new_app_component(
    component_name: PackageName,
    language: GuestLanguage,
) -> Result<GolemResult, GolemError> {
    let all_examples = all_composable_app_examples();

    let Some(language_examples) = all_examples.get(&language) else {
        return Err(GolemError(format!(
            "No template found for {}, currently supported languages: {}",
            language,
            all_examples.keys().join(", ")
        )));
    };

    let default_examples = language_examples
        .get(&ComposableAppGroupName::default())
        .expect("No default template found for the selected language");

    assert_eq!(
        default_examples.components.len(),
        1,
        "Expected exactly one default component template"
    );

    let default_component_example = &default_examples.components[0];

    match add_component_by_example(
        default_examples.common.as_ref(),
        default_component_example,
        &env::current_dir().expect("Failed to get current working directory"),
        &component_name,
    ) {
        Ok(_) => Ok(GolemResult::Str(format!(
            "Added new app component {}",
            component_name.to_string_with_colon()
        ))),
        Err(err) => Err(GolemError(format!("Failed to add component: {err}"))),
    }
}

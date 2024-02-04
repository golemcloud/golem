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

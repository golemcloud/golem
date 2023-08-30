use crate::GolemResult;
use golem_examples::model::{Example, ExampleName, ExampleParameters, GuestLanguage, GuestLanguageTier, PackageName, TemplateName};
use golem_examples::*;
use std::env;

pub fn process_new(example_name: ExampleName, template_name: TemplateName, package_name: Option<PackageName>) -> GolemResult {
    let examples = GolemExamples::list_all_examples();
    let example = examples
        .iter()
        .find(|example| example.name == example_name);
    match example {
        Some(example) => {
            let cwd = env::current_dir().expect("Failed to get current working directory");
            match GolemExamples::instantiate(
                example,
                ExampleParameters {
                    template_name: template_name.clone(),
                    package_name: package_name
                        .clone()
                        .unwrap_or(PackageName::from_string("golem:template").unwrap()),
                    target_path: cwd,
                },
            ) {
                Ok(instructions) => GolemResult::Str(format!("{instructions}")),
                Err(err) => GolemResult::Err(format!("Failed to instantiate template: {err}")),
            }
        }
        None => {
            GolemResult::Err(format!("Unknown template {example_name}. Use the list-templates command to see the available commands."))
        }
    }
}

pub fn process_list_templates(min_tier: &Option<GuestLanguageTier>, language: &Option<GuestLanguage>) -> GolemResult {
   let examples = GolemExamples::list_all_examples()
        .iter()
        .filter(|example| match language {
            Some(language) => example.language == *language,
            None => true,
        })
        .filter(|example| match min_tier {
            Some(min_tier) => example.language.tier() <= *min_tier,
            None => true,
        })
       .map(|r| r.clone())
        .collect::<Vec<Example>>();

    GolemResult::Ok(Box::new(examples))
}
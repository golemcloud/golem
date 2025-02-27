use clap::Parser;
use golem_examples::cli::*;
use golem_examples::model::*;
use golem_examples::{
    add_component_by_example, all_composable_app_examples, all_standalone_examples,
    instantiate_example,
};

pub fn main() {
    let command: GolemCommand = GolemCommand::parse();
    match &command.command {
        Command::New {
            name_or_language,
            component_name,
            package_name,
        } => {
            let example_name = name_or_language.example_name();
            let examples = all_standalone_examples();
            let example = examples.iter().find(|example| example.name == example_name);
            match example {
                Some(example) => {
                    let cwd =
                        std::env::current_dir().expect("Failed to get current working directory");
                    match instantiate_example(
                        example,
                        &ExampleParameters {
                            component_name: component_name.clone(),
                            package_name: package_name
                                .clone()
                                .unwrap_or(PackageName::from_string("golem:component").unwrap()),
                            target_path: cwd.join(component_name.as_str()),
                        },
                        TargetExistsResolveMode::Fail,
                    ) {
                        Ok(instructions) => println!("{instructions}"),
                        Err(err) => eprintln!("Failed to instantiate example: {err:?}"),
                    }
                }
                None => {
                    eprintln!("Unknown example {example_name}. Use the list-examples command to see the available commands.");
                }
            }
        }
        Command::ListExamples { min_tier, language } => {
            all_standalone_examples()
                .iter()
                .filter(|example| match language {
                    Some(language) => example.language == *language,
                    None => true,
                })
                .filter(|example| match min_tier {
                    Some(min_tier) => example.language.tier() <= *min_tier,
                    None => true,
                })
                .for_each(|example| println!("{:?}", example));
        }
        Command::ListAppExamples {
            language: language_filter,
            group: group_filter,
        } => {
            for (language, examples) in all_composable_app_examples() {
                if let Some(language_filter) = language_filter {
                    if language_filter != &language {
                        continue;
                    }
                }

                for (group, examples) in examples {
                    if let Some(group_filter) = group_filter {
                        if group_filter != &group {
                            continue;
                        }
                    }

                    if let Some(common) = examples.common {
                        println!("{language} - {group} - common");
                        println!("{:?}\n", common);
                    }
                    for example in examples.components.iter() {
                        println!("{language} - {group} - component");
                        println!("{:?}\n", example);
                    }
                }
            }
        }
        Command::NewAppComponent {
            component_name,
            language,
        } => {
            let all_examples = all_composable_app_examples();

            let language_examples = all_examples
                .get(language)
                .expect("No template found for the selected language");

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
                &std::env::current_dir().expect("Failed to get current working directory"),
                component_name,
            ) {
                Ok(_) => {}
                Err(err) => eprintln!("Failed to instantiate example: {err:?}"),
            }
        }
    }
}

use crate::{cargo, GenerateArgs};
use std::fs;
use std::process::Command;
use toml::map::Map;
use toml::Value;

pub fn initialize_workspace(
    targets: &[String],
    callers: &[String],
    wasm_rpc_path_override: Option<String>,
    stubgen_command: &str,
    stubgen_prefix: &[&str],
) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    if cargo::is_cargo_workspace_toml(&cwd.join("Cargo.toml"))? {
        let makefile_path = cwd.join("Makefile.toml");
        if makefile_path.exists() && makefile_path.is_file() {
            Err(anyhow::anyhow!("Makefile.toml already exists. Modifying existing cargo-make projects is currently not supported."))
        } else if has_cargo_make() {
            let makefile = generate_makefile(
                targets,
                callers,
                wasm_rpc_path_override.clone(),
                stubgen_command,
                stubgen_prefix,
            )?;
            println!("Writing cargo-make Makefile to {:?}", makefile_path);
            fs::write(makefile_path, makefile)?;

            for target in targets {
                println!("Generating initial stub for {target}");

                crate::generate(GenerateArgs {
                    source_wit_root: cwd.join(format!("{target}/wit")),
                    dest_crate_root: cwd.join(format!("{target}-stub")),
                    world: None,
                    stub_crate_version: "0.0.1".to_string(),
                    wasm_rpc_path_override: wasm_rpc_path_override.clone(),
                })?;
            }

            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "cargo-make is not installed. Please install it with `cargo install cargo-make`"
            ))
        }
    } else {
        Err(anyhow::anyhow!("Not in a cargo workspace"))
    }
}

fn generate_makefile(
    targets: &[String],
    callers: &[String],
    wasm_rpc_path_override: Option<String>,
    stubgen_command: &str,
    stubgen_prefix: &[&str],
) -> anyhow::Result<String> {
    let mut root = Map::default();

    let mut config = Map::default();
    config.insert("default_to_workspace".to_string(), Value::Boolean(false));
    root.insert("config".to_string(), Value::Table(config));

    let mut tasks = Map::default();

    let mut default = Map::default();
    default.insert("alias".to_string(), Value::String("build".to_string()));
    tasks.insert("default".to_string(), Value::Table(default));

    let mut clean = Map::default();
    clean.insert(
        "command".to_string(),
        Value::String("cargo-component".to_string()),
    );
    clean.insert(
        "args".to_string(),
        Value::Array(vec![Value::String("clean".to_string())]),
    );
    tasks.insert("clean".to_string(), Value::Table(clean));

    let mut build = Map::default();
    build.insert(
        "command".to_string(),
        Value::String("cargo-component".to_string()),
    );
    build.insert(
        "args".to_string(),
        Value::Array(vec![Value::String("build".to_string())]),
    );
    build.insert(
        "dependencies".to_string(),
        Value::Array(vec![
            Value::String("clean".to_string()),
            Value::String("regenerate-stubs".to_string()),
        ]),
    );
    tasks.insert("build".to_string(), Value::Table(build));

    let mut build_release = Map::default();
    build_release.insert(
        "command".to_string(),
        Value::String("cargo-component".to_string()),
    );
    build_release.insert(
        "args".to_string(),
        Value::Array(vec![
            Value::String("build".to_string()),
            Value::String("--release".to_string()),
        ]),
    );
    build_release.insert(
        "dependencies".to_string(),
        Value::Array(vec![
            Value::String("clean".to_string()),
            Value::String("regenerate-stubs".to_string()),
        ]),
    );
    tasks.insert("build-release".to_string(), Value::Table(build_release));

    let mut test = Map::default();
    test.insert(
        "command".to_string(),
        Value::String("cargo-component".to_string()),
    );
    test.insert(
        "args".to_string(),
        Value::Array(vec![Value::String("test".to_string())]),
    );
    test.insert(
        "dependencies".to_string(),
        Value::Array(vec![Value::String("clean".to_string())]),
    );
    tasks.insert("test".to_string(), Value::Table(test));

    let mut regenerate_stub_tasks = Vec::new();
    for target in targets {
        let mut generate_stub = Map::default();
        generate_stub.insert("cwd".to_string(), Value::String(".".to_string()));
        generate_stub.insert(
            "command".to_string(),
            Value::String(stubgen_command.to_string()),
        );
        let mut args = Vec::new();
        args.extend(stubgen_prefix.iter().map(|s| Value::String(s.to_string())));
        args.push(Value::String("generate".to_string()));
        args.push(Value::String("-s".to_string()));
        args.push(Value::String(format!("{}/wit", target)));
        args.push(Value::String("-d".to_string()));
        args.push(Value::String(format!("{}-stub", target)));
        if let Some(wasm_rpc_path_override) = wasm_rpc_path_override.as_ref() {
            args.push(Value::String("--wasm-rpc-path-override".to_string()));
            args.push(Value::String(wasm_rpc_path_override.to_string()));
        }
        generate_stub.insert("args".to_string(), Value::Array(args));

        let generate_stub_task_name = format!("generate-{}-stub", target);
        tasks.insert(generate_stub_task_name.clone(), Value::Table(generate_stub));

        for caller in callers {
            let mut add_stub_dependency = Map::default();
            add_stub_dependency.insert("cwd".to_string(), Value::String(".".to_string()));
            add_stub_dependency.insert(
                "command".to_string(),
                Value::String(stubgen_command.to_string()),
            );

            let mut args = Vec::new();
            args.extend(stubgen_prefix.iter().map(|s| Value::String(s.to_string())));
            args.extend(vec![
                Value::String("add-stub-dependency".to_string()),
                Value::String("--stub-wit-root".to_string()),
                Value::String(format!("{}-stub/wit", target)),
                Value::String("--dest-wit-root".to_string()),
                Value::String(format!("{}/wit", caller)),
                Value::String("--overwrite".to_string()),
                Value::String("--update-cargo-toml".to_string()),
            ]);

            add_stub_dependency.insert("args".to_string(), Value::Array(args));
            add_stub_dependency.insert(
                "dependencies".to_string(),
                Value::Array(vec![Value::String(generate_stub_task_name.clone())]),
            );

            let regenerate_task_name = format!("add-stub-dependency-{}-{}", target, caller);
            tasks.insert(
                regenerate_task_name.clone(),
                Value::Table(add_stub_dependency),
            );
            regenerate_stub_tasks.push(regenerate_task_name);
        }
    }

    let mut regenerate_stubs = Map::default();
    regenerate_stubs.insert(
        "dependencies".to_string(),
        Value::Array(
            regenerate_stub_tasks
                .iter()
                .map(|n| Value::String(n.to_string()))
                .collect::<Vec<_>>(),
        ),
    );
    tasks.insert(
        "regenerate-stubs".to_string(),
        Value::Table(regenerate_stubs),
    );

    let mut compose_tasks = Vec::new();
    let mut compose_release_tasks = Vec::new();
    for caller in callers {
        let mut compose = Map::default();
        compose.insert("cwd".to_string(), Value::String(".".to_string()));
        compose.insert(
            "command".to_string(),
            Value::String(stubgen_command.to_string()),
        );
        let mut args = Vec::new();
        args.extend(stubgen_prefix.iter().map(|s| Value::String(s.to_string())));
        args.push(Value::String("compose".to_string()));
        args.push(Value::String("--source-wasm".to_string()));
        args.push(Value::String(format!(
            "target/wasm32-wasi/debug/{}.wasm",
            caller
        )));
        for target in targets {
            args.push(Value::String("--stub-wasm".to_string()));
            args.push(Value::String(format!(
                "target/wasm32-wasi/debug/{}_stub.wasm",
                target
            )));
        }
        args.push(Value::String("--dest-wasm".to_string()));
        args.push(Value::String(format!(
            "target/wasm32-wasi/debug/{}-composed.wasm",
            caller
        )));

        compose.insert("args".to_string(), Value::Array(args));

        let task_name = format!("compose-{}", caller);
        tasks.insert(task_name.clone(), Value::Table(compose));
        compose_tasks.push(task_name);

        let mut compose_release = Map::default();
        compose_release.insert("cwd".to_string(), Value::String(".".to_string()));
        compose_release.insert(
            "command".to_string(),
            Value::String(stubgen_command.to_string()),
        );
        let mut args = Vec::new();
        args.extend(stubgen_prefix.iter().map(|s| Value::String(s.to_string())));
        args.push(Value::String("compose".to_string()));
        args.push(Value::String("--source-wasm".to_string()));
        args.push(Value::String(format!(
            "target/wasm32-wasi/release/{}.wasm",
            caller
        )));
        for target in targets {
            args.push(Value::String("--stub-wasm".to_string()));
            args.push(Value::String(format!(
                "target/wasm32-wasi/release/{}_stub.wasm",
                target
            )));
        }
        args.push(Value::String("--dest-wasm".to_string()));
        args.push(Value::String(format!(
            "target/wasm32-wasi/release/{}-composed.wasm",
            caller
        )));

        compose_release.insert("args".to_string(), Value::Array(args));

        let task_name = format!("compose-release-{}", caller);
        tasks.insert(task_name.clone(), Value::Table(compose_release));
        compose_release_tasks.push(task_name);
    }

    let mut post_build = Map::default();
    post_build.insert(
        "dependencies".to_string(),
        Value::Array(
            compose_tasks
                .iter()
                .map(|n| Value::String(n.to_string()))
                .collect::<Vec<_>>(),
        ),
    );
    tasks.insert("post-build".to_string(), Value::Table(post_build));

    let mut post_build_release = Map::default();
    post_build_release.insert(
        "dependencies".to_string(),
        Value::Array(
            compose_release_tasks
                .iter()
                .map(|n| Value::String(n.to_string()))
                .collect::<Vec<_>>(),
        ),
    );
    tasks.insert(
        "post-build-release".to_string(),
        Value::Table(post_build_release),
    );

    let mut build_flow = Map::default();
    build_flow.insert(
        "dependencies".to_string(),
        Value::Array(vec![
            Value::String("build".to_string()),
            Value::String("post-build".to_string()),
        ]),
    );
    tasks.insert("build-flow".to_string(), Value::Table(build_flow));

    let mut release_build_flow = Map::default();
    release_build_flow.insert(
        "dependencies".to_string(),
        Value::Array(vec![
            Value::String("build-release".to_string()),
            Value::String("post-build-release".to_string()),
        ]),
    );
    tasks.insert(
        "release-build-flow".to_string(),
        Value::Table(release_build_flow),
    );

    root.insert("tasks".to_string(), Value::Table(tasks));

    let result = toml::to_string(&root)?;
    Ok(result)
}

fn has_cargo_make() -> bool {
    Command::new("cargo-make")
        .args(["--version"])
        .output()
        .is_ok()
}

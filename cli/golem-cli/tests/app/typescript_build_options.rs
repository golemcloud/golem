use crate::Tracing;
use crate::app::{TestContext, cmd, flag, merge_into_manifest};
use golem_cli::fs;
use indoc::formatdoc;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(Tracing);

#[test]
#[timeout("15 minutes")]
async fn typescript_readable_build_presets_preserve_behavior_and_invalidate_bundles() {
    let mut ctx = TestContext::new();
    ctx.start_server().await;
    assert!(
        ctx.cli([
            flag::YES,
            cmd::NEW,
            "readable-build",
            flag::TEMPLATE,
            "ts",
            flag::COMPONENT_NAME,
            "readable-build:main",
        ])
        .await
        .success_or_dump()
    );
    ctx.cd("readable-build");
    fs::write_str(
        ctx.cwd_path_join("src/counter-agent.ts"),
        include_str!("typescript_build_options.ts"),
    )
    .unwrap();
    let mut minified = None;
    let mut bundle_path = None;
    for (index, preset) in ["quick", "debug", "quick"].iter().enumerate() {
        assert!(
            ctx.cli([flag::YES, "-P", preset, cmd::DEPLOY])
                .await
                .success_or_dump()
        );
        let bundles = std::fs::read_dir(ctx.cwd_path_join("golem-temp/ts-dist"))
            .unwrap()
            .map(|entry| entry.unwrap().path().join("main.js"))
            .filter(|path| path.is_file())
            .collect::<Vec<_>>();
        assert_eq!(bundles.len(), 1);
        let path = &bundles[0];
        let code = fs::read_to_string(path).unwrap();
        if *preset == "debug" {
            assert!(code.contains("applicationDiagnosticMessage"));
            assert_ne!(Some(&code), minified.as_ref());
        } else {
            assert!(!code.contains("applicationDiagnosticMessage"));
            if let Some(previous) = &minified {
                assert_eq!(&code, previous);
            }
            minified = Some(code);
        }
        assert!(
            !std::fs::read_dir(ctx.cwd_path_join("golem-temp/agents"))
                .unwrap()
                .any(|entry| entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".preinitialized.wasm"))
        );
        // Settle strict source/target timestamp comparisons, then prove a fresh
        // unchanged build preserves both the bundle bytes and its timestamp.
        assert!(
            ctx.cli([flag::YES, "-P", preset, cmd::BUILD])
                .await
                .success_or_dump()
        );
        let before = std::fs::metadata(path).unwrap().modified().unwrap();
        let contents = std::fs::read(path).unwrap();
        assert!(
            ctx.cli([flag::YES, "-P", preset, cmd::BUILD])
                .await
                .success_or_dump()
        );
        assert_eq!(std::fs::metadata(path).unwrap().modified().unwrap(), before);
        assert_eq!(std::fs::read(path).unwrap(), contents);
        let agent = format!("CounterAgent(\"{preset}-{index}\")");
        let value = ctx.cli([cmd::AGENT, cmd::INVOKE, &agent, "value"]).await;
        assert!(value.success_or_dump());
        assert!(value.stdout_contains("readable application success"));
        let failure = ctx.cli([cmd::AGENT, cmd::INVOKE, &agent, "fail"]).await;
        assert!(!failure.success());
        assert!(
            failure.stdout_contains("readable application failure")
                || failure.stderr_contains("readable application failure")
        );
        println!("{preset}: {} application JS bytes", contents.len());
        bundle_path = Some(path.clone());
    }

    let bundle = bundle_path.unwrap();
    let component_name = bundle
        .parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();
    let dynamic = std::fs::read_dir(ctx.cwd_path_join("golem-temp/agents"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.to_string_lossy().ends_with(".dynamic.wasm"))
        .unwrap();
    let dynamic = serde_json::to_string(dynamic.to_str().unwrap()).unwrap();
    let final_wasm = std::fs::read_dir(ctx.cwd_path_join("golem-temp/agents"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "wasm")
                && !path.to_string_lossy().ends_with(".dynamic.wasm")
        })
        .unwrap();
    let final_wasm = serde_json::to_string(final_wasm.to_str().unwrap()).unwrap();
    let app_root = serde_json::to_string(ctx.cwd_path().to_str().unwrap()).unwrap();
    let temp = serde_json::to_string(ctx.cwd_path_join("golem-temp").to_str().unwrap()).unwrap();
    // A component-level preset is authored configuration. Its paths are concrete;
    // template-only variables are not expanded in component properties.
    let authored = formatdoc! {r#"
        components:
          readable-build:main:
            presets:
              invalid:
                buildMergeMode: replace
                componentWasm: {dynamic}
                outputWasm: {final_wasm}
                build:
                - command: npx --no rollup -- -c golem-temp/common/ts/rollup.config.component.mjs
                  dir: .
                  env:
                    GOLEM_COMPONENT_MINIFY: invalid
                    GOLEM_APP_ROOT: {app_root}
                    GOLEM_TEMP: {temp}
                    GOLEM_COMPONENT_NAME: {component_name}
                  sources: [src, tsconfig.json, package.json, golem-temp/common/ts/rollup.config.component.mjs]
                  targets: [golem-temp/ts-dist/{component_name}/main.js]
    "#};
    merge_into_manifest(&ctx.cwd_path_join("golem.yaml"), &authored).unwrap();
    let invalid = ctx.cli([flag::YES, "-P", "invalid", cmd::BUILD]).await;
    assert!(!invalid.success());
    assert!(
        invalid.stdout_contains("GOLEM_COMPONENT_MINIFY must be")
            || invalid.stderr_contains("GOLEM_COMPONENT_MINIFY must be")
    );
    let implicit = authored
        .replace("invalid:", "implicit:")
        .replace("GOLEM_COMPONENT_MINIFY: invalid", "");
    merge_into_manifest(&ctx.cwd_path_join("golem.yaml"), &implicit).unwrap();
    assert!(
        ctx.cli([flag::YES, "-P", "implicit", cmd::BUILD, flag::STEP, "build"])
            .await
            .success_or_dump()
    );
    assert_eq!(fs::read_to_string(&bundle).unwrap(), minified.unwrap());
}

fn dependency_install_count(ctx: &TestContext) -> u32 {
    fs::read_to_string(ctx.cwd_path_join(".install-count"))
        .unwrap()
        .parse()
        .unwrap()
}

fn dependency_output_state(paths: &[std::path::PathBuf]) -> Vec<(Vec<u8>, std::time::SystemTime)> {
    paths
        .iter()
        .map(|path| {
            (
                std::fs::read(path).unwrap(),
                std::fs::metadata(path).unwrap().modified().unwrap(),
            )
        })
        .collect()
}

async fn assert_dependency_build_unchanged(ctx: &TestContext, outputs: &[std::path::PathBuf]) {
    let count = dependency_install_count(ctx);
    let before = dependency_output_state(outputs);
    assert!(
        ctx.cli([flag::YES, "-P", "quick", cmd::BUILD])
            .await
            .success_or_dump()
    );
    assert_eq!(dependency_install_count(ctx), count);
    assert_eq!(dependency_output_state(outputs), before);
}

#[test]
#[timeout("20 minutes")]
async fn typescript_and_effect_builds_prepare_dependencies_before_freshness_checks() {
    for template in ["ts", "effect"] {
        let mut ctx = TestContext::new();
        ctx.add_env_var("NPM_CONFIG_IGNORE_SCRIPTS", "false");
        ctx.add_env_var("npm_config_ignore_scripts", "false");
        ctx.start_server().await;
        assert!(
            ctx.cli([
                flag::YES,
                cmd::NEW,
                "dependency-build",
                flag::TEMPLATE,
                template,
                flag::COMPONENT_NAME,
                "dependency-build:main",
            ])
            .await
            .success_or_dump()
        );
        ctx.cd("dependency-build");
        fs::write_str(
            ctx.cwd_path_join("count-install.cjs"),
            "const fs = require('node:fs'); const path = '.install-count';\n\
             const count = fs.existsSync(path) ? Number(fs.readFileSync(path, 'utf8')) : 0;\n\
             fs.writeFileSync(path, String(count + 1));\n",
        )
        .unwrap();
        let package_path = ctx.cwd_path_join("package.json");
        let mut package: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&package_path).unwrap()).unwrap();
        let existing = package["scripts"]["postinstall"].as_str().unwrap_or("");
        let script = if existing.is_empty() {
            "node count-install.cjs".to_string()
        } else {
            format!("{existing} && node count-install.cjs")
        };
        if !package["scripts"].is_object() {
            package["scripts"] = serde_json::json!({});
        }
        package["scripts"]["postinstall"] = script.into();
        std::fs::write(&package_path, serde_json::to_vec_pretty(&package).unwrap()).unwrap();
        assert!(
            ctx.cli([flag::YES, "-P", "quick", cmd::BUILD])
                .await
                .success_or_dump()
        );
        let mut outputs = Vec::new();
        for (directory, file) in [("ts-dist", "main.js"), ("ts-check", "main.d.ts")] {
            if directory == "ts-check" && template == "ts" {
                continue;
            }
            let matches = std::fs::read_dir(ctx.cwd_path_join(format!("golem-temp/{directory}")))
                .unwrap()
                .map(|entry| entry.unwrap().path().join(file))
                .filter(|path| path.is_file())
                .collect::<Vec<_>>();
            assert_eq!(matches.len(), 1, "{template}: {directory}");
            outputs.extend(matches);
        }
        let package = std::fs::read(&package_path).unwrap();
        let source_path = ctx.cwd_path_join("src/counter-agent.ts");
        let source = std::fs::read(&source_path).unwrap();
        assert_dependency_build_unchanged(&ctx, &outputs).await;

        let lock_path = ctx.cwd_path_join("package-lock.json");
        let mut lock: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&lock_path).unwrap()).unwrap();
        lock["name"] = "lock-only-change".into();
        std::fs::write(&lock_path, serde_json::to_vec_pretty(&lock).unwrap()).unwrap();
        for delete_lock in [false, true] {
            if delete_lock {
                std::fs::remove_file(&lock_path).unwrap();
            }
            let count = dependency_install_count(&ctx);
            let before = dependency_output_state(&outputs);
            assert!(
                ctx.cli([flag::YES, "-P", "quick", cmd::BUILD])
                    .await
                    .success_or_dump()
            );
            assert_eq!(dependency_install_count(&ctx), count + 1, "{template}");
            assert!(lock_path.is_file());
            for (previous, current) in before.iter().zip(dependency_output_state(&outputs)) {
                assert_ne!(previous.1, current.1, "{template}: output was not rebuilt");
            }
            assert_dependency_build_unchanged(&ctx, &outputs).await;
        }

        let count = dependency_install_count(&ctx);
        std::fs::remove_dir_all(ctx.cwd_path_join("node_modules")).unwrap();
        assert!(
            ctx.cli([flag::YES, "-P", "quick", cmd::BUILD])
                .await
                .success_or_dump()
        );
        assert_eq!(dependency_install_count(&ctx), count + 1, "{template}");
        assert!(ctx.cwd_path_join("node_modules").is_dir());
        assert_dependency_build_unchanged(&ctx, &outputs).await;
        fs::write_str(ctx.cwd_path_join(".npmrc"), "package-lock=false\n").unwrap();
        assert!(
            ctx.cli([flag::YES, "-P", "quick", cmd::BUILD])
                .await
                .success_or_dump()
        );
        assert!(lock_path.is_file());
        let count = dependency_install_count(&ctx);
        let before = dependency_output_state(&outputs);
        std::fs::remove_file(&lock_path).unwrap();
        assert!(
            ctx.cli([flag::YES, "-P", "quick", cmd::BUILD])
                .await
                .success_or_dump()
        );
        assert_eq!(dependency_install_count(&ctx), count + 1, "{template}");
        assert!(!lock_path.exists(), "npm must honor package-lock=false");
        for (previous, current) in before.iter().zip(dependency_output_state(&outputs)) {
            assert_ne!(
                previous.1, current.1,
                "{template}: removed input was ignored"
            );
        }
        assert_dependency_build_unchanged(&ctx, &outputs).await;

        let agents = std::fs::read_dir(ctx.cwd_path_join("golem-temp/agents"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        let dynamic = agents
            .iter()
            .find(|path| path.to_string_lossy().ends_with(".dynamic.wasm"))
            .unwrap();
        let output = agents
            .iter()
            .find(|path| {
                path.extension().is_some_and(|ext| ext == "wasm")
                    && !path.to_string_lossy().ends_with(".dynamic.wasm")
            })
            .unwrap();
        let dynamic = serde_json::to_string(dynamic.to_str().unwrap()).unwrap();
        let output = serde_json::to_string(output.to_str().unwrap()).unwrap();
        let cleanup = formatdoc! {r#"
            components:
              dependency-build:main:
                presets:
                  dependency-cleanup:
                    buildMergeMode: replace
                    componentWasm: {dynamic}
                    outputWasm: {output}
                    build:
                    - command: node -e "require.resolve('typescript')"
                      dir: .
                    - command: node -e "require.resolve('typescript')"
                      dir: .
                      rmdirs: [node_modules]
        "#};
        merge_into_manifest(&ctx.cwd_path_join("golem.yaml"), &cleanup).unwrap();
        let count = dependency_install_count(&ctx);
        assert!(
            ctx.cli([
                flag::YES,
                "-P",
                "dependency-cleanup",
                cmd::BUILD,
                flag::STEP,
                "build"
            ])
            .await
            .success_or_dump()
        );
        assert_eq!(
            dependency_install_count(&ctx),
            count + 1,
            "{template}: rmdirs must invalidate the earlier ensure"
        );
        assert!(ctx.cwd_path_join("node_modules").is_dir());
        assert_dependency_build_unchanged(&ctx, &outputs).await;
        assert_eq!(std::fs::read(package_path).unwrap(), package);
        assert_eq!(std::fs::read(source_path).unwrap(), source);
    }
}

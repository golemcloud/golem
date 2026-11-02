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

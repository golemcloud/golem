use crate::app::{TestContext, cmd, extracted_component_metadata_path_hash, flag};
use base64::Engine;
use golem_cli::{fs, versions};
use serde_json::{Value, json};
use sha2::{Digest, Sha512};
use std::path::Path;
use test_r::{test, timeout};

fn npm_archive(
    root: &Path,
    name: &str,
    version: &str,
    source: &str,
    declarations: &str,
    dependencies: Value,
) -> Value {
    let file = format!("{name}-{version}.tgz");
    let mut archive = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::default(),
    ));
    let manifest = serde_json::to_vec(&json!({
        "name": name, "version": version, "type": "module", "main": "index.js",
        "types": "index.d.ts", "dependencies": dependencies,
    }))
    .unwrap();
    for (name, bytes) in [
        ("package.json", manifest.as_slice()),
        ("index.js", source.as_bytes()),
        ("index.d.ts", declarations.as_bytes()),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        archive
            .append_data(&mut header, format!("package/{name}"), bytes)
            .unwrap();
    }
    let bytes = archive.into_inner().unwrap().finish().unwrap();
    fs::write(root.join(&file), &bytes).unwrap();
    json!({
        "version": version, "resolved": format!("file:{file}"),
        "integrity": format!("sha512-{}", base64::engine::general_purpose::STANDARD.encode(Sha512::digest(bytes))),
        "dependencies": dependencies,
    })
}

#[test]
#[timeout("15m")]
async fn typescript_and_effect_builds_refresh_lock_only_dependencies_and_bundles() {
    for language in ["ts", "effect"] {
        let mut ctx = TestContext::new();
        assert!(
            ctx.cli([
                flag::YES,
                cmd::NEW,
                "lock-inputs",
                flag::TEMPLATE,
                language,
                flag::COMPONENT_NAME,
                "app:main"
            ])
            .await
            .success_or_dump()
        );
        ctx.cd("lock-inputs");
        fs::write_str(ctx.cwd_path_join("golem.yaml"), format!(
            "manifestVersion: {}\napp: lock-inputs\nenvironments:\n  local: {{server: local, componentPresets: quick}}\ncomponents:\n  app:main:\n    templates: {language}\n    dir: .\n", versions::sdk::MANIFEST
        )).unwrap();
        assert!(
            ctx.cli([flag::YES, cmd::BUILD, flag::STEP, "build"])
                .await
                .success_or_dump()
        );
        let root = ctx.cwd_path_join("");
        let a = npm_archive(
            &root,
            "leaf",
            "1.0.0",
            "export default 'locked-value-A';",
            "declare const value: string; export default value;",
            json!({}),
        );
        let b = npm_archive(
            &root,
            "leaf",
            "1.1.0",
            "export default 'locked-value-B';",
            "declare const value: string; export default value;",
            json!({}),
        );
        let wrapper = npm_archive(
            &root,
            "wrapper",
            "1.0.0",
            "export {default} from 'leaf';",
            "export {default} from 'leaf';",
            json!({"leaf": "^1.0.0"}),
        );
        let package_path = root.join("package.json");
        let mut package: Value =
            serde_json::from_str(&fs::read_to_string(&package_path).unwrap()).unwrap();
        package["dependencies"]["wrapper"] = json!("file:wrapper-1.0.0.tgz");
        let package_bytes = serde_json::to_vec_pretty(&package).unwrap();
        fs::write(&package_path, &package_bytes).unwrap();
        let lock_path = root.join("package-lock.json");
        let mut lock: Value =
            serde_json::from_str(&fs::read_to_string(&lock_path).unwrap()).unwrap();
        lock["packages"][""]["dependencies"]["wrapper"] = json!("file:wrapper-1.0.0.tgz");
        lock["packages"]["node_modules/wrapper"] = wrapper;
        lock["packages"]["node_modules/leaf"] = a.clone();
        fs::write(&lock_path, serde_json::to_vec_pretty(&lock).unwrap()).unwrap();
        let source = if language == "ts" {
            "import value from 'wrapper';\nimport {z} from 'zod';\nimport {defineAgent, method} from '@golemcloud/golem-ts-sdk';\ndefineAgent({name:'LockConsumer', id:{name:z.string()}, methods:{read:method({input:{},returns:z.string()})}}).implement({init:()=>({}),methods:{read:()=>value}});\n"
        } else {
            "import value from 'wrapper';\nimport {Effect, Schema} from 'effect';\nimport {defineAgent, method} from '@golemcloud/effect-golem';\ndefineAgent({name:'LockConsumer',id:{name:Schema.String},methods:{read:method({input:{},success:Schema.String})}}).implement({init:()=>Effect.succeed({}),methods:()=>({read:()=>Effect.succeed(value)})});\n"
        };
        fs::write_str(root.join("src/counter-agent.ts"), source).unwrap();
        let bundle = root.join("golem-temp/ts-dist/app-main/main.js");
        let declarations = root.join("golem-temp/ts-check/app-main/main.d.ts");
        for (entry, expected, installed_externally) in [
            (a.clone(), "locked-value-A", false),
            (b, "locked-value-B", false),
            (a, "locked-value-A", true),
        ] {
            let mut lock: Value =
                serde_json::from_str(&fs::read_to_string(&lock_path).unwrap()).unwrap();
            lock["packages"]["node_modules/leaf"] = entry;
            fs::write(&lock_path, serde_json::to_vec_pretty(&lock).unwrap()).unwrap();
            if installed_externally {
                let output = tokio::process::Command::new("npm")
                    .current_dir(&root)
                    .args(["install", "--offline", "--no-audit", "--no-fund"])
                    .output()
                    .await
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            let before_typecheck = declarations.exists().then(|| {
                std::fs::metadata(&declarations)
                    .unwrap()
                    .modified()
                    .unwrap()
            });
            assert!(
                ctx.cli([flag::YES, cmd::BUILD, flag::STEP, "build"])
                    .await
                    .success_or_dump()
            );
            let bytes = fs::read_to_string(&bundle).unwrap();
            assert!(
                bytes.contains(expected),
                "{language}: bundle has the wrong dependency result"
            );
            assert!(
                fs::read_to_string(root.join("node_modules/leaf/index.js"))
                    .unwrap()
                    .contains(expected)
            );
            assert_eq!(fs::read(&package_path).unwrap(), package_bytes);
            assert_eq!(
                fs::read_to_string(root.join("src/counter-agent.ts")).unwrap(),
                source
            );
            if let Some(before) = before_typecheck {
                assert!(
                    std::fs::metadata(&declarations)
                        .unwrap()
                        .modified()
                        .unwrap()
                        > before
                );
            }
            let modified = std::fs::metadata(&bundle).unwrap().modified().unwrap();
            let typecheck_modified = declarations.exists().then(|| {
                std::fs::metadata(&declarations)
                    .unwrap()
                    .modified()
                    .unwrap()
            });
            assert!(
                ctx.cli([flag::YES, cmd::BUILD, flag::STEP, "build"])
                    .await
                    .success_or_dump()
            );
            assert_eq!(fs::read_to_string(&bundle).unwrap(), bytes);
            assert_eq!(
                std::fs::metadata(&bundle).unwrap().modified().unwrap(),
                modified
            );
            assert_eq!(
                declarations
                    .exists()
                    .then(|| std::fs::metadata(&declarations)
                        .unwrap()
                        .modified()
                        .unwrap()),
                typecheck_modified
            );
        }
    }
}

#[test]
#[timeout("15m")]
async fn both_typescript_presets_recheck_consumers_when_generated_guest_contracts_change() {
    for preset in ["quick", "optimized"] {
        let mut ctx = TestContext::new();
        assert!(
            ctx.cli([
                flag::YES,
                cmd::NEW,
                "guest-inputs",
                flag::TEMPLATE,
                "ts",
                flag::COMPONENT_NAME,
                "app:provider"
            ])
            .await
            .success_or_dump()
        );
        ctx.cd("guest-inputs");
        assert!(
            ctx.cli([
                flag::YES,
                cmd::NEW,
                ".",
                flag::TEMPLATE,
                "ts",
                flag::COMPONENT_NAME,
                "app:consumer"
            ])
            .await
            .success_or_dump()
        );
        fs::write_str(ctx.cwd_path_join("golem.yaml"), format!(
            "manifestVersion: {}\napp: guest-inputs\nenvironments:\n  local: {{server: local, componentPresets: {preset}}}\ncomponents:\n  app:provider:\n    dir: provider\n    templates: ts\n  app:consumer:\n    dir: consumer\n    templates: ts\n    dependencies:\n      agents: [app:provider/FreshProvider]\n", versions::sdk::MANIFEST
        )).unwrap();
        for name in ["provider", "consumer"] {
            fs::create_dir_all(ctx.cwd_path_join(format!("{name}/src"))).unwrap();
            fs::write_str(
                ctx.cwd_path_join(format!("{name}/tsconfig.json")),
                include_str!("../../templates/ts/component/component-dir/tsconfig.json"),
            )
            .unwrap();
        }
        let config_path = ctx.cwd_path_join("consumer/tsconfig.json");
        let mut config: Value =
            serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
        config["compilerOptions"]["paths"]["fresh-provider-guest-client"] = json!([
            "../golem-temp/bridge-sdk/ts/internal/fresh-provider-guest-client/fresh-provider-guest-client.ts"
        ]);
        // The client is a transitive import, not an include-glob input.
        fs::write(&config_path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
        let consumer = "import {z} from 'zod';\nimport {defineAgent, method} from '@golemcloud/golem-ts-sdk';\nimport type {FreshProvider} from 'fresh-provider-guest-client';\ntype ProviderResult = Awaited<ReturnType<FreshProvider['read']>>;\nconst result: ProviderResult = {oldResult:'value'};\nexport const Consumer = defineAgent({name:'FreshConsumer',id:{name:z.string()},methods:{read:method({input:{},returns:z.string()})}});\nConsumer.implement({init:()=>({}),methods:{read:()=> result.oldResult}});\n";
        fs::write_str(ctx.cwd_path_join("consumer/src/main.ts"), consumer).unwrap();
        let provider_path = ctx.cwd_path_join("provider/src/main.ts");
        let provider = |field: &str| {
            format!(
                "import {{z}} from 'zod';\nimport {{defineAgent, method}} from '@golemcloud/golem-ts-sdk';\nexport const Provider = defineAgent({{name:'FreshProvider',id:{{name:z.string()}},methods:{{read:method({{input:{{}},returns:z.object({{{field}:z.string()}})}})}}}});\nProvider.implement({{init:()=>({{}}),methods:{{read:()=>({{{field}:'value'}})}}}});\n"
            )
        };
        fs::write_str(&provider_path, provider("oldResult")).unwrap();
        assert!(ctx.cli([flag::YES, cmd::BUILD]).await.success_or_dump());
        let client_path = ctx.cwd_path_join("golem-temp/bridge-sdk/ts/internal/fresh-provider-guest-client/fresh-provider-guest-client.ts");
        let old_client = fs::read(&client_path).unwrap();
        let bundle_path = ctx.cwd_path_join("golem-temp/ts-dist/app-consumer/main.js");
        let bundle = fs::read(&bundle_path).unwrap();
        let modified = std::fs::metadata(&bundle_path).unwrap().modified().unwrap();
        let package = fs::read(ctx.cwd_path_join("package.json")).unwrap();
        let lock = fs::read(ctx.cwd_path_join("package-lock.json")).unwrap();
        assert!(ctx.cli([flag::YES, cmd::BUILD]).await.success_or_dump());
        assert_eq!(fs::read(&client_path).unwrap(), old_client);
        assert_eq!(fs::read(&bundle_path).unwrap(), bundle);
        assert_eq!(
            std::fs::metadata(&bundle_path).unwrap().modified().unwrap(),
            modified
        );
        fs::write_str(&provider_path, provider("newResult")).unwrap();
        let output = ctx.cli([flag::YES, cmd::BUILD]).await;
        assert!(
            !output.success(),
            "{preset}: an incompatible generated contract was skipped"
        );
        assert!(output.stdout_contains("oldResult") || output.stderr_contains("oldResult"));
        assert!(
            output.stdout_contains("does not exist") || output.stderr_contains("does not exist")
        );
        let new_client = fs::read(&client_path).unwrap();
        assert_ne!(new_client, old_client);
        assert!(String::from_utf8(new_client).unwrap().contains("newResult"));
        assert_eq!(
            fs::read_to_string(ctx.cwd_path_join("consumer/src/main.ts")).unwrap(),
            consumer
        );
        assert_eq!(
            fs::read(ctx.cwd_path_join("package.json")).unwrap(),
            package
        );
        assert_eq!(
            fs::read(ctx.cwd_path_join("package-lock.json")).unwrap(),
            lock
        );
    }
}

#[test]
#[timeout("10m")]
async fn injection_configuration_switches_restore_the_original_guest_behavior() {
    let mut ctx = TestContext::new();
    assert!(
        ctx.cli([
            flag::YES,
            cmd::NEW,
            "injection-owner",
            flag::TEMPLATE,
            "ts",
            flag::COMPONENT_NAME,
            "app:main"
        ])
        .await
        .success_or_dump()
    );
    ctx.cd("injection-owner");
    for name in ["OwnerA", "OwnerB"] {
        fs::write_str(ctx.cwd_path_join("src/counter-agent.ts"), format!(
            "import {{z}} from 'zod';\nimport {{defineAgent, method}} from '@golemcloud/golem-ts-sdk';\nexport const Owner = defineAgent({{name:'{name}',id:{{name:z.string()}},methods:{{read:method({{input:{{}},returns:z.string()}})}}}});\nOwner.implement({{init:()=>({{}}),methods:{{read:()=>'{name}'}}}});\n"
        )).unwrap();
        assert!(
            ctx.cli([flag::YES, cmd::BUILD, "-P", "quick", flag::STEP, "build"])
                .await
                .success_or_dump()
        );
        fs::copy(
            ctx.cwd_path_join("golem-temp/ts-dist/app-main/main.js"),
            ctx.cwd_path_join(format!("{name}.mjs")),
        )
        .unwrap();
    }
    assert_ne!(
        fs::read(ctx.cwd_path_join("OwnerA.mjs")).unwrap(),
        fs::read(ctx.cwd_path_join("OwnerB.mjs")).unwrap()
    );
    let wasm = ctx.cwd_path_join("shared.wasm");
    let hash = extracted_component_metadata_path_hash(&ctx, "app:main", &wasm);
    let metadata = ctx.cwd_path_join(format!(
        "golem-temp/extracted-component-metadata/app:main-{hash}.json"
    ));
    for name in ["OwnerA", "OwnerB", "OwnerA"] {
        fs::write_str(ctx.cwd_path_join("golem.yaml"), format!(
            "manifestVersion: {}\napp: injection-owner\nenvironments:\n  local: {{server: local}}\ncomponents:\n  app:main:\n    componentWasm: shared.wasm\n    build:\n      - injectToPrebuiltQuickjs: node_modules/@golemcloud/golem-ts-sdk/wasm/agent_guest.wasm\n        module: {name}.mjs\n        into: shared.wasm\nbridge:\n  rust:\n    external:\n      agents: '*'\n", versions::sdk::MANIFEST
        )).unwrap();
        for repeat in 0..2 {
            let before = wasm
                .exists()
                .then(|| std::fs::metadata(&wasm).unwrap().modified().unwrap());
            assert!(ctx.cli([flag::YES, cmd::BUILD]).await.success_or_dump());
            let reflected: golem_common::model::agent::extraction::ExtractedComponentMetadata =
                serde_json::from_str(&fs::read_to_string(&metadata).unwrap()).unwrap();
            assert_eq!(
                reflected
                    .agent_types
                    .iter()
                    .map(|agent| agent.type_name.as_str())
                    .collect::<Vec<_>>(),
                [name]
            );
            if repeat == 1 {
                assert_eq!(
                    Some(std::fs::metadata(&wasm).unwrap().modified().unwrap()),
                    before
                );
            }
        }
    }
}

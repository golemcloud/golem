// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

//! Effect-native TypeScript guest tool client generation.

use super::tool::TypeScriptToolBridgeGenerator;
use crate::bridge_gen::tool_bridge_client_directory_name;
use crate::sdk_overrides::sdk_overrides;
use anyhow::Context;
use camino::{Utf8Path, Utf8PathBuf};
use golem_common::schema::tool::Tool;
use serde_json::json;

const EFFECT_VERSION: &str = "4.0.0-beta.98";

/// Generates the exact graph-backed TypeScript tool client and binds it to the Effect transport.
pub struct EffectToolBridgeGenerator {
    tool: Tool,
    tool_name: String,
    target_path: Utf8PathBuf,
    testing: bool,
}

impl EffectToolBridgeGenerator {
    pub fn new(tool: Tool, target_path: &Utf8Path, testing: bool) -> anyhow::Result<Self> {
        let tool_name = tool
            .name()
            .context("tool command tree must contain a root command")?
            .to_string();
        Ok(Self {
            tool,
            tool_name,
            target_path: target_path.to_path_buf(),
            testing,
        })
    }

    pub fn generate(&mut self) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.target_path)?;
        let package_name = tool_bridge_client_directory_name(&self.tool_name);
        let mut canonical =
            TypeScriptToolBridgeGenerator::new(self.tool.clone(), &self.target_path, self.testing)?;
        let root = canonical.root_client_class_name()?.to_string();
        let source = canonical.source_effect()?;
        let source = format!(
            "{source}\n/** Effect client using the same structural codecs and canonical projection. */\nexport const client = base.client({root}, {});\n",
            serde_json::to_string(&self.tool_name)?
        );
        std::fs::write(self.target_path.join(format!("{package_name}.ts")), source)?;

        let effect_dep = sdk_overrides()?.effect_golem_dep()?;
        let package = json!({
            "name": package_name, "version": "0.0.1", "type": "module",
            "main": format!("{package_name}.js"), "types": format!("{package_name}.d.ts"),
            "scripts": { "build": "tsc" },
            "dependencies": { "@golemcloud/effect-golem": effect_dep, "effect": EFFECT_VERSION },
            "devDependencies": { "typescript": "^5.9", "@types/node": "^25" }
        });
        std::fs::write(
            self.target_path.join("package.json"),
            serde_json::to_string_pretty(&package)?,
        )?;
        let tsconfig = json!({ "compilerOptions": {
            "target": "es2020", "module": "esnext", "moduleResolution": "bundler",
            "strict": true, "declaration": true, "skipLibCheck": true
        }, "include": [format!("{package_name}.ts")] });
        std::fs::write(
            self.target_path.join("tsconfig.json"),
            serde_json::to_string_pretty(&tsconfig)?,
        )?;
        Ok(())
    }
}

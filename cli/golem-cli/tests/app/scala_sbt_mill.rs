// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! S3: one Scala app, built with the sbt plugin and with the Mill plugin.
//!
//! The scenario builds each preset once and collects every assertion, so one failure does not
//! hide the others. Later Scala rows (failed preinitialization, computed and unbundled
//! `js.import`) add their checks to `scala_app_built_with_sbt_and_mill`.

use super::{TestContext, cmd, flag};
use crate::workspace_path;
use sha2::{Digest, Sha256};
use std::path::Path;
use test_r::{test, timeout};

const APP_NAME: &str = "scala-sbt-mill";

#[derive(Default)]
struct Failures(Vec<String>);

impl Failures {
    fn check(&mut self, ok: bool, message: impl FnOnce() -> String) {
        if !ok {
            self.0.push(message());
        }
    }

    fn assert_none(self) {
        assert!(
            self.0.is_empty(),
            "S3 failed {} check(s):\n- {}",
            self.0.len(),
            self.0.join("\n- ")
        );
    }
}

fn bundle(path: &Path) -> Option<(usize, String)> {
    std::fs::read(path).ok().map(|bytes| {
        let digest = Sha256::digest(&bytes);
        let hash = digest.iter().map(|b| format!("{b:02x}")).collect();
        (bytes.len(), hash)
    })
}

#[test]
#[timeout("30 minutes")]
async fn scala_app_built_with_sbt_and_mill() {
    let mut ctx = TestContext::new();
    fs_extra::dir::copy(
        ctx.test_data_path_join(APP_NAME),
        ctx.cwd_path(),
        &fs_extra::dir::CopyOptions::new(),
    )
    .unwrap();
    ctx.cd(APP_NAME);

    // The Mill plugin is not published, so the app's meta-build compiles it from this checkout.
    let sdk_dir = workspace_path().join("sdks/scala");
    ctx.add_env_var("GOLEM_SCALA_SDK_DIR", sdk_dir.to_str().unwrap());

    ctx.start_server().await;
    let mut failures = Failures::default();

    let sbt_build = ctx.cli([flag::YES, cmd::BUILD, flag::FORCE_BUILD]).await;
    failures.check(sbt_build.success_or_dump(), || {
        "golem build with the sbt preset failed".to_string()
    });
    let mill_build = ctx
        .cli(["-P", "mill", flag::YES, cmd::BUILD, flag::FORCE_BUILD])
        .await;
    failures.check(mill_build.success_or_dump(), || {
        "golem build with the mill preset failed".to_string()
    });

    let sbt_bundle = bundle(&ctx.cwd_path_join(".golem/sbt.js"));
    let mill_bundle = bundle(&ctx.cwd_path_join(".golem/mill.js"));
    failures.check(sbt_bundle.is_some(), || "sbt wrote no bundle".to_string());
    failures.check(mill_bundle.is_some(), || "Mill wrote no bundle".to_string());
    if let (Some(sbt), Some(mill)) = (&sbt_bundle, &mill_bundle) {
        failures.check(sbt == mill, || {
            format!(
                "sbt and Mill bundles differ: sbt {} bytes sha256 {}, Mill {} bytes sha256 {}",
                sbt.0, sbt.1, mill.0, mill.1
            )
        });
    }

    let deployed = ctx.cli(["-P", "mill", flag::YES, cmd::DEPLOY]).await;
    failures.check(deployed.success_or_dump(), || {
        "golem deploy of the Mill-built app failed".to_string()
    });
    if deployed.success() {
        let agent_types = ctx.cli([cmd::AGENT_TYPE, cmd::LIST]).await;
        failures.check(
            agent_types.success_or_dump() && agent_types.stdout_contains("BuildToolAgent"),
            || "the Mill-built component does not expose the BuildToolAgent agent type".to_string(),
        );

        let invoked = ctx
            .cli([
                flag::YES,
                cmd::AGENT,
                cmd::INVOKE,
                "BuildToolAgent(\"s3\")",
                "describe",
                "[3, 1, 2]",
            ])
            .await;
        failures.check(
            invoked.success_or_dump() && invoked.stdout_contains("\"s3:6:3\""),
            || {
                "BuildToolAgent.describe on the Mill-built agent did not return \"s3:6:3\""
                    .to_string()
            },
        );
    }

    failures.assert_none();
}

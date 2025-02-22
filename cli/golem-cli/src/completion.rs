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

use clap::Command;

pub fn print_completion(mut command: Command, shell: clap_complete::Shell) {
    let cmd_name = command.get_name().to_string();
    tracing::info!("Golem CLI - generating completion file for {cmd_name} - {shell:?}...");
    clap_complete::generate(shell, &mut command, cmd_name, &mut std::io::stdout());
}

// Copyright 2024 Golem Cloud
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

use lenient_bool::LenientBool;

pub mod clients;
pub mod cloud;
pub mod command;
pub mod config;
pub mod connect_output;
pub mod diagnose;
pub mod examples;
pub mod factory;
pub mod init;
pub mod model;
pub mod oss;
pub mod service;
pub mod stubgen;

pub fn parse_key_val(
    s: &str,
) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let pos = s
        .find('=')
        .ok_or_else(|| format!("invalid KEY=value: no `=` found in `{s}`"))?;
    Ok((s[..pos].parse()?, s[pos + 1..].parse()?))
}

pub fn parse_bool(s: &str) -> Result<bool, Box<dyn std::error::Error + Send + Sync + 'static>> {
    match s.parse::<LenientBool>() {
        Ok(b) => Ok(b.into()),
        Err(_) => Err(format!("invalid boolean: `{s}`"))?,
    }
}

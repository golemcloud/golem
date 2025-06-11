// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use golem_wasm_ast::analysis::AnalysedType;
use rib::RegistryKey;
use std::fmt::{Debug, Display, Formatter};

#[derive(Debug)]
pub struct ConflictingFunction {
    pub function: RegistryKey,
    pub parameter_type_conflict: Option<ParameterTypeConflict>,
    pub return_type_conflict: Option<ReturnTypeConflict>,
}

#[derive(Debug)]
pub struct ParameterTypeConflict {
    pub existing: Vec<AnalysedType>,
    pub new: Vec<AnalysedType>,
}

#[derive(Debug)]
pub struct ReturnTypeConflict {
    pub existing: Option<AnalysedType>,
    pub new: Option<AnalysedType>,
}

impl Display for ConflictingFunction {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Function: {}", self.function)?;

        match self.parameter_type_conflict {
            Some(ref conflict) => {
                writeln!(f, "  Parameter Type Conflict:")?;
                writeln!(
                    f,
                    "    Existing: {}",
                    convert_to_pretty_types(&conflict.existing)
                )?;
                writeln!(
                    f,
                    "    New:      {}",
                    convert_to_pretty_types(&conflict.new)
                )?;
            }
            None => {
                writeln!(f, "  Parameter Type Conflict: None")?;
            }
        }

        match self.return_type_conflict {
            Some(ref conflict) => {
                writeln!(f, "  Result Type Conflict:")?;
                writeln!(
                    f,
                    "    Existing: {}",
                    convert_to_pretty_type(&conflict.existing)
                )?;
                writeln!(f, "    New:      {}", convert_to_pretty_type(&conflict.new))?;
            }
            None => {
                writeln!(f, "  Result Type Conflict: None")?;
            }
        }

        Ok(())
    }
}

fn convert_to_pretty_types(analysed_types: &[AnalysedType]) -> String {
    let type_names = analysed_types
        .iter()
        .map(|x| {
            rib::TypeName::try_from(x.clone()).map_or("unknown".to_string(), |x| x.to_string())
        })
        .collect::<Vec<_>>();

    type_names.join(", ")
}

fn convert_to_pretty_type(analysed_type: &Option<AnalysedType>) -> String {
    analysed_type
        .as_ref()
        .map(|x| {
            rib::TypeName::try_from(x.clone()).map_or("unknown".to_string(), |x| x.to_string())
        })
        .unwrap_or("unit".to_string())
}

#[derive(Debug)]
pub struct ConflictReport {
    pub missing_functions: Vec<RegistryKey>,
    pub conflicting_functions: Vec<ConflictingFunction>,
}

impl Display for ConflictReport {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        // Handling missing functions
        writeln!(f, "Missing Functions:")?;
        if self.missing_functions.is_empty() {
            writeln!(f, "  None")?;
        } else {
            for missing_function in &self.missing_functions {
                writeln!(f, "  - {}", missing_function)?;
            }
        }

        // Handling conflicting functions
        writeln!(f, "\nFunctions with conflicting types:")?;
        if self.conflicting_functions.is_empty() {
            writeln!(f, "  None")?;
        } else {
            for conflict in &self.conflicting_functions {
                writeln!(f, "{}", conflict)?;
            }
        }

        Ok(())
    }
}

impl ConflictReport {
    pub fn is_empty(&self) -> bool {
        self.missing_functions.is_empty() && self.conflicting_functions.is_empty()
    }
}

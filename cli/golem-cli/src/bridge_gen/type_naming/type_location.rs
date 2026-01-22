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

use std::fmt::{Display, Formatter};

#[derive(Debug, Clone)]
pub struct TypeLocation {
    pub root: TypeLocationRoot,
    pub path: Option<TypeLocationPath>,
}

impl TypeLocation {
    pub fn to_type_naming_segments(&self) -> Vec<Vec<&str>> {
        match &self.root {
            TypeLocationRoot::AgentConstructorInput { input_name } => {
                let mut segments = vec![vec!["ConstructorInput", input_name.as_str()]];
                if let Some(path) = &self.path {
                    segments.extend(path.to_type_naming_segments());
                }
                segments
            }
            TypeLocationRoot::AgentMethodInput {
                method_name,
                input_name,
            } => {
                let mut segments = vec![vec![method_name.as_str(), "Input", input_name.as_str()]];
                if let Some(path) = &self.path {
                    segments.extend(path.to_type_naming_segments());
                }
                segments
            }
            TypeLocationRoot::AgentMethodOutput {
                method_name,
                output_name,
            } => {
                let mut segments = vec![vec![method_name.as_str(), "Output", output_name.as_str()]];
                if let Some(path) = &self.path {
                    segments.extend(path.to_type_naming_segments());
                }
                segments
            }
        }
    }
}

impl Display for TypeLocation {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.root)?;
        if let Some(step) = &self.path {
            write!(f, "{}", step)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)]
pub enum TypeLocationRoot {
    AgentConstructorInput {
        input_name: String,
    },
    AgentMethodInput {
        method_name: String,
        input_name: String,
    },
    AgentMethodOutput {
        method_name: String,
        output_name: String,
    },
}

impl Display for TypeLocationRoot {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            TypeLocationRoot::AgentConstructorInput { input_name } => {
                write!(f, "AgentConstructorInput<{}>", input_name)
            }
            TypeLocationRoot::AgentMethodInput {
                method_name,
                input_name,
            } => write!(f, "AgentMethodInput<{}, {}>", method_name, input_name),
            TypeLocationRoot::AgentMethodOutput {
                method_name,
                output_name,
            } => write!(f, "AgentMethodOutput<{}, {}>", method_name, output_name),
        }
    }
}

#[derive(Debug, Clone)]
pub enum TypeLocationPath {
    VariantCase {
        name: Option<String>,
        owner: Option<String>,
        case: String,
        inner: Option<Box<TypeLocationPath>>,
    },
    ResultOk {
        name: Option<String>,
        owner: Option<String>,
        inner: Option<Box<TypeLocationPath>>,
    },
    ResultErr {
        name: Option<String>,
        owner: Option<String>,
        inner: Option<Box<TypeLocationPath>>,
    },
    Option {
        name: Option<String>,
        owner: Option<String>,
        inner: Option<Box<TypeLocationPath>>,
    },
    RecordField {
        name: Option<String>,
        owner: Option<String>,
        field_name: String,
        inner: Option<Box<TypeLocationPath>>,
    },
    TupleItem {
        name: Option<String>,
        owner: Option<String>,
        idx: String,
        inner: Option<Box<TypeLocationPath>>,
    },
    List {
        name: Option<String>,
        owner: Option<String>,
        inner: Option<Box<TypeLocationPath>>,
    },
}

impl TypeLocationPath {
    pub fn inner_mut(&mut self) -> &mut Option<Box<TypeLocationPath>> {
        match self {
            TypeLocationPath::VariantCase { inner, .. } => inner,
            TypeLocationPath::ResultOk { inner, .. } => inner,
            TypeLocationPath::ResultErr { inner, .. } => inner,
            TypeLocationPath::Option { inner, .. } => inner,
            TypeLocationPath::RecordField { inner, .. } => inner,
            TypeLocationPath::TupleItem { inner, .. } => inner,
            TypeLocationPath::List { inner, .. } => inner,
        }
    }

    pub fn to_type_naming_segments(&self) -> Vec<Vec<&str>> {
        fn collect<'a>(segments: &mut Vec<Vec<&'a str>>, path: &'a TypeLocationPath) {
            match path {
                TypeLocationPath::VariantCase {
                    name,
                    owner,
                    case,
                    inner,
                } => {
                    let mut subsegments = vec![];
                    if let Some(owner) = owner {
                        subsegments.push(owner.as_str());
                    }
                    if let Some(name) = name {
                        subsegments.push(name.as_str());
                    }
                    subsegments.push(case.as_str());
                    segments.push(subsegments);
                    if let Some(inner) = inner {
                        collect(segments, inner.as_ref());
                    }
                }
                TypeLocationPath::ResultOk { name, owner, inner } => {
                    let mut subsegments = vec![];
                    if let Some(owner) = owner {
                        subsegments.push(owner.as_str());
                    }
                    if let Some(name) = name {
                        subsegments.push(name.as_str());
                    }
                    segments.push(subsegments);
                    if let Some(inner) = inner {
                        collect(segments, inner.as_ref());
                    }
                }
                TypeLocationPath::ResultErr { name, owner, inner } => {
                    let mut subsegments = vec![];
                    if let Some(owner) = owner {
                        subsegments.push(owner.as_str());
                    }
                    if let Some(name) = name {
                        subsegments.push(name.as_str());
                    }
                    segments.push(subsegments);
                    if let Some(inner) = inner {
                        collect(segments, inner.as_ref());
                    }
                }
                TypeLocationPath::Option { name, owner, inner } => {
                    let mut subsegments = vec![];
                    if let Some(owner) = owner {
                        subsegments.push(owner.as_str());
                    }
                    if let Some(name) = name {
                        subsegments.push(name.as_str());
                    }
                    segments.push(subsegments);
                    if let Some(inner) = inner {
                        collect(segments, inner.as_ref());
                    }
                }
                TypeLocationPath::RecordField {
                    name,
                    owner,
                    field_name,
                    inner,
                } => {
                    let mut subsegments = vec![];
                    if let Some(owner) = owner {
                        subsegments.push(owner.as_str());
                    }
                    if let Some(name) = name {
                        subsegments.push(name.as_str());
                    }
                    subsegments.push(field_name.as_str());
                    segments.push(subsegments);
                    if let Some(inner) = inner {
                        collect(segments, inner.as_ref());
                    }
                }
                TypeLocationPath::TupleItem {
                    name,
                    owner,
                    idx,
                    inner,
                } => {
                    let mut subsegments = vec![];
                    if let Some(owner) = owner {
                        subsegments.push(owner.as_str());
                    }
                    if let Some(name) = name {
                        subsegments.push(name.as_str());
                    }
                    subsegments.push(idx.as_str());
                    segments.push(subsegments);
                    if let Some(inner) = inner {
                        collect(segments, inner.as_ref());
                    }
                }
                TypeLocationPath::List { name, owner, inner } => {
                    let mut subsegments = vec![];
                    if let Some(owner) = owner {
                        subsegments.push(owner.as_str());
                    }
                    if let Some(name) = name {
                        subsegments.push(name.as_str());
                    }
                    segments.push(subsegments);
                    if let Some(inner) = inner {
                        collect(segments, inner.as_ref());
                    }
                }
            }
        }

        let mut segments = vec![];
        collect(&mut segments, self);
        segments
    }
}

impl Display for TypeLocationPath {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        fn write_name(
            f: &mut Formatter<'_>,
            name: &Option<String>,
            owner: &Option<String>,
        ) -> std::fmt::Result {
            match (name, owner) {
                (Some(name), Some(owner)) => write!(f, "<{}::{}>", owner, name),
                (Some(name), None) => write!(f, "<{}>", name),
                (None, Some(owner)) => write!(f, "<{}::???>", owner),
                (None, None) => Ok(()),
            }
        }

        match &self {
            TypeLocationPath::VariantCase {
                name,
                owner,
                case,
                inner,
            } => {
                write!(f, "/Variant")?;
                write_name(f, name, owner)?;
                write!(f, ".{}", case)?;
                if let Some(inner) = inner {
                    write!(f, "{}", inner)?;
                }
            }
            TypeLocationPath::ResultOk {
                name,
                owner,
                inner: ok,
            } => {
                write!(f, "/ResultOk")?;
                write_name(f, name, owner)?;
                if let Some(ok) = ok {
                    write!(f, "{}", ok)?;
                }
            }
            TypeLocationPath::ResultErr {
                name,
                owner,
                inner: err,
            } => {
                write!(f, "/ResultErr")?;
                write_name(f, name, owner)?;
                if let Some(err) = err {
                    write!(f, "{}", err)?;
                }
            }
            TypeLocationPath::Option { name, owner, inner } => {
                write!(f, "/Option")?;
                write_name(f, name, owner)?;
                if let Some(inner) = inner {
                    write!(f, "{}", inner)?;
                }
            }
            TypeLocationPath::RecordField {
                owner,
                name,
                field_name,
                inner: field,
            } => {
                write!(f, "/RecordField")?;
                write_name(f, name, owner)?;
                write!(f, ".{}", field_name)?;
                if let Some(field) = field {
                    write!(f, "{}", field)?;
                }
            }
            TypeLocationPath::TupleItem {
                owner,
                name,
                idx,
                inner: item,
            } => {
                write!(f, "/Tuple")?;
                write_name(f, name, owner)?;
                write!(f, ".{}", idx)?;
                if let Some(item) = item {
                    write!(f, "{}", item)?;
                }
            }
            TypeLocationPath::List { owner, name, inner } => {
                write!(f, "/List")?;
                write_name(f, name, owner)?;
                if let Some(inner) = inner {
                    write!(f, "{}", inner)?;
                }
            }
        }

        Ok(())
    }
}

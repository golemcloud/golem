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

use crate::core::{
    CoreIndexSpace, CoreSectionType, Custom, Data, Expr, ExprSource, RetainsCustomSection,
    TryFromExprSource,
};
use crate::Section;
use std::fmt::Debug;

/// A trait for customizing some of the data types used in the WASM AST.
///
/// Three types are customizable:
/// - `Expr`: The type of the expression nodes in the AST, holding sequence of WASM instructions.
/// - `Data`: The type of the data section in the AST.
/// - `Custom`: The type of the custom section in the AST.
///
/// Use one of the predefined customization types or create your own:
/// - [DefaultAst] uses the real AST nodes, keeping all parsed information
/// - [IgnoreAll] ignores all instructions, custom sections and data sections
/// - [IgnoreAllButMetadata] ignores all instructions, data sections and custom sections except those that hold information parsable by the `wasm-metadata` crate.
pub trait AstCustomization: Debug + Clone + PartialEq {
    type Expr: Debug + Clone + PartialEq;
    type Data: Debug + Clone + PartialEq + Section<CoreIndexSpace, CoreSectionType>;

    #[cfg(not(feature = "component"))]
    type Custom: Debug + Clone + PartialEq + Section<CoreIndexSpace, CoreSectionType>;
    #[cfg(feature = "component")]
    type Custom: Debug
        + Clone
        + PartialEq
        + Section<CoreIndexSpace, CoreSectionType>
        + Section<crate::component::ComponentIndexSpace, crate::component::ComponentSectionType>;
}

/// The default AST customization, using the real AST nodes.
#[derive(Debug, Clone, PartialEq)]
pub struct DefaultAst;

impl AstCustomization for DefaultAst {
    type Expr = Expr;
    type Data = Data<Expr>;
    type Custom = Custom;
}

#[derive(Debug, Clone, PartialEq)]
pub struct IgnoredExpr;

impl TryFromExprSource for IgnoredExpr {
    fn try_from<S: ExprSource>(_value: S) -> Result<Self, String>
    where
        Self: Sized,
    {
        Ok(IgnoredExpr)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct IgnoredData;

impl From<Data<IgnoredExpr>> for IgnoredData {
    fn from(_value: Data<IgnoredExpr>) -> Self {
        IgnoredData
    }
}

impl Section<CoreIndexSpace, CoreSectionType> for IgnoredData {
    fn index_space(&self) -> CoreIndexSpace {
        CoreIndexSpace::Data
    }

    fn section_type(&self) -> CoreSectionType {
        CoreSectionType::Data
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct IgnoredCustom;

impl From<Custom> for IgnoredCustom {
    fn from(_value: Custom) -> Self {
        IgnoredCustom
    }
}

impl Section<CoreIndexSpace, CoreSectionType> for IgnoredCustom {
    fn index_space(&self) -> CoreIndexSpace {
        CoreIndexSpace::Custom
    }

    fn section_type(&self) -> CoreSectionType {
        CoreSectionType::Custom
    }
}

#[cfg(feature = "component")]
impl Section<crate::component::ComponentIndexSpace, crate::component::ComponentSectionType>
    for IgnoredCustom
{
    fn index_space(&self) -> crate::component::ComponentIndexSpace {
        crate::component::ComponentIndexSpace::Custom
    }

    fn section_type(&self) -> crate::component::ComponentSectionType {
        crate::component::ComponentSectionType::Custom
    }
}

/// An AST customization that ignores all instructions, custom sections and data sections.
#[derive(Debug, Clone, PartialEq)]
pub struct IgnoreAll;

impl AstCustomization for IgnoreAll {
    type Expr = IgnoredExpr;
    type Data = IgnoredData;
    type Custom = IgnoredCustom;
}

#[derive(Debug, Clone, PartialEq)]
pub enum MetadataOnlyCustom {
    Metadata(Custom),
    Ignored,
}

impl RetainsCustomSection for MetadataOnlyCustom {
    fn name(&self) -> &str {
        match self {
            MetadataOnlyCustom::Metadata(custom) => custom.name(),
            MetadataOnlyCustom::Ignored => "ignored",
        }
    }

    fn data(&self) -> &[u8] {
        match self {
            MetadataOnlyCustom::Metadata(custom) => custom.data(),
            MetadataOnlyCustom::Ignored => &[],
        }
    }
}

impl From<Custom> for MetadataOnlyCustom {
    fn from(value: Custom) -> Self {
        if value.name == "producers"
            || value.name == "registry-metadata"
            || value.name == "name"
            || value.name == "component-name"
        {
            MetadataOnlyCustom::Metadata(value)
        } else {
            MetadataOnlyCustom::Ignored
        }
    }
}

impl Section<CoreIndexSpace, CoreSectionType> for MetadataOnlyCustom {
    fn index_space(&self) -> CoreIndexSpace {
        CoreIndexSpace::Custom
    }

    fn section_type(&self) -> CoreSectionType {
        CoreSectionType::Custom
    }
}

#[cfg(feature = "component")]
impl Section<crate::component::ComponentIndexSpace, crate::component::ComponentSectionType>
    for MetadataOnlyCustom
{
    fn index_space(&self) -> crate::component::ComponentIndexSpace {
        crate::component::ComponentIndexSpace::Custom
    }

    fn section_type(&self) -> crate::component::ComponentSectionType {
        crate::component::ComponentSectionType::Custom
    }
}

/// An AST customization that ignores all instructions, data sections and custom sections except those that hold information parsable by the `wasm-metadata` crate.
#[derive(Debug, Clone, PartialEq)]
pub struct IgnoreAllButMetadata;

impl AstCustomization for IgnoreAllButMetadata {
    type Expr = IgnoredExpr;
    type Data = IgnoredData;
    type Custom = MetadataOnlyCustom;
}

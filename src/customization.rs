use crate::core::{
    CoreIndexSpace, CoreSectionType, Custom, Data, Expr, ExprSource, TryFromExprSource,
};
use crate::Section;
use std::fmt::Debug;

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

#[derive(Debug, Clone, PartialEq)]
pub struct IgnoreAll;

impl AstCustomization for IgnoreAll {
    type Expr = IgnoredExpr;
    type Data = IgnoredData;
    type Custom = IgnoredCustom;
}

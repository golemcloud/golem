use std::collections::{BTreeMap, BTreeSet};
use serde::{Deserialize, Serialize};
use crate::wit_config::wit_types::WitType;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WitConfig {
    pub package: String,
    pub interfaces: BTreeSet<Interface>,
    pub world: World,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct World {
    pub name: String,
    pub uses: Vec<UseStatement>,
    pub imports: Vec<Interface>,
    pub exports: Vec<Interface>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, derive_setters::Setters)]
#[setters(into)]
pub struct Interface {
    pub name: String,
    pub varients: BTreeMap<String, WitType>,
    pub records: BTreeSet<Record>,
    pub uses: Vec<UseStatement>,
    pub functions: Vec<Function>,
}

impl PartialOrd for Interface {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.name.cmp(&other.name))
    }
}

impl Ord for Interface {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.name.cmp(&other.name)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub name: String,
    pub fields: BTreeSet<Field>,
    pub added_fields: BTreeSet<Field>,
}

impl PartialOrd for Record {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.name.cmp(&other.name))
    }
}

impl Ord for Record {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.name.cmp(&other.name)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UseStatement {
    pub name: String,
    pub items: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Function {
    pub name: String,
    pub parameters: Vec<Parameter>,
    pub return_type: ReturnTy,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReturnTy {
    pub return_type: String,
    pub error_type: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parameter {
    pub name: String,
    pub parameter_type: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    pub field_type: WitType,
}

impl PartialOrd for Field {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.name.cmp(&other.name))
    }
}

impl Ord for Field {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.name.cmp(&other.name)
    }
}

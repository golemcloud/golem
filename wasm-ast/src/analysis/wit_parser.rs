use crate::analysis::{analysed_type, AnalysedType, NameOptionTypePair, NameTypePair};
use itertools::Itertools;
use std::collections::HashMap;
use std::fmt::Debug;
use std::path::Path;
use std::sync::{Arc, Mutex};
use wit_parser::{Resolve, Type, TypeDef, TypeDefKind};
use wit_parser::{TypeId, TypeOwner as WitParserTypeOwner};

/// ToAnalysedType converts a Type or TypeDef from a wit_parser::Resolve.
///
/// ToAnalysedType is intended to be used for helping with writing tests where AnalysedType
/// have to be constructed in the test. For simpler values and types this is usually
/// not a problem, but creating more complex nested or variant types manually can be convoluted.
///
/// Note that resources and handles are not implemented.
pub trait ToAnalysedType {
    fn to_analysed_type(&self, resolve: &Resolve) -> Result<AnalysedType, String>;
}

impl ToAnalysedType for TypeDef {
    fn to_analysed_type(&self, resolve: &Resolve) -> Result<AnalysedType, String> {
        match &self.kind {
            TypeDefKind::Record(record) => Ok(analysed_type::record(
                record
                    .fields
                    .iter()
                    .map(|field| {
                        field.ty.to_analysed_type(resolve).map(|typ| NameTypePair {
                            name: field.name.clone(),
                            typ,
                        })
                    })
                    .collect::<Result<_, _>>()?,
            )),
            TypeDefKind::Resource => {
                Err("to_analysed_type not implemented for resource type".to_string())
            }

            TypeDefKind::Handle(_) => {
                Err("to_analysed_type not implemented for handle type".to_string())
            }
            TypeDefKind::Flags(flag) => Ok(analysed_type::flags(
                &flag
                    .flags
                    .iter()
                    .map(|flag| flag.name.as_str())
                    .collect::<Vec<_>>(),
            )),
            TypeDefKind::Tuple(tuple) => Ok(analysed_type::tuple(
                tuple
                    .types
                    .iter()
                    .map(|typ| typ.to_analysed_type(resolve))
                    .collect::<Result<_, _>>()?,
            )),
            TypeDefKind::Variant(variant) => Ok(analysed_type::variant(
                variant
                    .cases
                    .iter()
                    .map(|case| {
                        case.ty
                            .map(|ty| ty.to_analysed_type(resolve))
                            .transpose()
                            .map(|ty| NameOptionTypePair {
                                name: case.name.clone(),
                                typ: ty,
                            })
                    })
                    .collect::<Result<_, _>>()?,
            )),
            TypeDefKind::Enum(enum_) => Ok(analysed_type::r#enum(
                &enum_
                    .cases
                    .iter()
                    .map(|case| case.name.as_str())
                    .collect::<Vec<_>>(),
            )),
            TypeDefKind::Option(inner) => {
                Ok(analysed_type::option(inner.to_analysed_type(resolve)?))
            }
            TypeDefKind::Result(result) => match (result.ok, result.err) {
                (Some(ok), Some(err)) => Ok(analysed_type::result(
                    ok.to_analysed_type(resolve)?,
                    err.to_analysed_type(resolve)?,
                )),
                (Some(ok), None) => Ok(analysed_type::result_ok(ok.to_analysed_type(resolve)?)),
                (None, Some(err)) => Ok(analysed_type::result_err(err.to_analysed_type(resolve)?)),
                (None, None) => Err("result type with no ok or err case".to_string()),
            },
            TypeDefKind::List(ty) => Ok(analysed_type::list(ty.to_analysed_type(resolve)?)),
            TypeDefKind::Future(_) => {
                Err("to_analysed_type not implemented for future type".to_string())
            }
            TypeDefKind::Stream(_) => {
                Err("to_analysed_type not implemented for stream type".to_string())
            }
            TypeDefKind::Type(typ) => typ.to_analysed_type(resolve),
            TypeDefKind::Unknown => Err("to_analysed_type unknown type".to_string()),
        }
    }
}

impl ToAnalysedType for Type {
    fn to_analysed_type(&self, resolve: &Resolve) -> Result<AnalysedType, String> {
        match self {
            Type::Bool => Ok(analysed_type::bool()),
            Type::U8 => Ok(analysed_type::u8()),
            Type::U16 => Ok(analysed_type::u16()),
            Type::U32 => Ok(analysed_type::u32()),
            Type::U64 => Ok(analysed_type::u64()),
            Type::S8 => Ok(analysed_type::s8()),
            Type::S16 => Ok(analysed_type::s16()),
            Type::S32 => Ok(analysed_type::s32()),
            Type::S64 => Ok(analysed_type::s64()),
            Type::F32 => Ok(analysed_type::f32()),
            Type::F64 => Ok(analysed_type::f64()),
            Type::Char => Ok(analysed_type::chr()),
            Type::String => Ok(analysed_type::str()),
            Type::Id(id) => resolve
                .types
                .get(*id)
                .ok_or_else(|| format!("Type not found by id: {:?}", id))?
                .to_analysed_type(resolve),
        }
    }
}

#[derive(Debug, Hash, PartialEq, Eq)]
pub enum TypeOwner {
    World(String),
    Interface(String),
    InlineInterface,
    None,
}

#[derive(Debug, Hash, PartialEq, Eq)]
pub struct TypeName {
    pub package: Option<String>,
    pub owner: TypeOwner,
    pub name: Option<String>,
}

pub struct AnalysedTypeResolve {
    resolve: Resolve,
    type_name_to_id: HashMap<TypeName, TypeId>,
    id_to_analysed_type: HashMap<TypeId, AnalysedType>,
}

impl Debug for AnalysedTypeResolve {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AnalysedTypeResolve")
    }
}

impl AnalysedTypeResolve {
    pub fn new(resolve: Resolve) -> Self {
        let type_name_to_id = resolve
            .types
            .iter()
            .map(|(type_id, type_def)| {
                (
                    match &type_def.owner {
                        WitParserTypeOwner::World(world_id) => {
                            let world = resolve.worlds.get(*world_id).unwrap();
                            TypeName {
                                package: world
                                    .package
                                    .and_then(|package_id| resolve.packages.get(package_id))
                                    .map(|package| package.name.to_string()),
                                owner: TypeOwner::World(world.name.clone()),
                                name: type_def.name.clone(),
                            }
                        }
                        WitParserTypeOwner::Interface(interface_id) => {
                            let interface = resolve.interfaces.get(*interface_id).unwrap();
                            TypeName {
                                package: interface
                                    .package
                                    .and_then(|package_id| resolve.packages.get(package_id))
                                    .map(|package| package.name.to_string()),
                                owner: {
                                    match &interface.name {
                                        Some(name) => TypeOwner::Interface(name.clone()),
                                        None => TypeOwner::InlineInterface,
                                    }
                                },
                                name: type_def.name.clone(),
                            }
                        }
                        WitParserTypeOwner::None => TypeName {
                            package: None,
                            owner: TypeOwner::None,
                            name: type_def.name.clone(),
                        },
                    },
                    type_id,
                )
            })
            .collect::<HashMap<_, _>>();

        AnalysedTypeResolve {
            resolve,
            type_name_to_id,
            id_to_analysed_type: HashMap::new(),
        }
    }

    pub fn from_wit_directory(directory: &Path) -> Result<Self, String> {
        let mut resolve = Resolve::new();
        resolve.push_dir(directory).map_err(|e| e.to_string())?;
        Ok(Self::new(resolve))
    }

    pub fn from_wit_str(wit: &str) -> Result<Self, String> {
        let mut resolve = Resolve::new();
        resolve
            .push_str(wit, "wit.wit")
            .map_err(|e| e.to_string())?;
        Ok(Self::new(resolve))
    }

    pub fn analysed_type(&mut self, name: &TypeName) -> Result<AnalysedType, String> {
        match self.type_name_to_id.get(name) {
            Some(type_id) => match self.id_to_analysed_type.get(type_id) {
                Some(typ) => Ok(typ.clone()),
                None => {
                    let typ = self
                        .resolve
                        .types
                        .get(*type_id)
                        .unwrap()
                        .to_analysed_type(&self.resolve)?;
                    self.id_to_analysed_type.insert(*type_id, typ.clone());
                    Ok(typ)
                }
            },
            None => Err(format!(
                "Type not found by name: {:?}, available types: {}",
                name,
                {
                    self.type_name_to_id
                        .keys()
                        .map(|type_id| format!("{type_id:?}"))
                        .join("\n")
                }
            )),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SharedAnalysedTypeResolve {
    resolve: Arc<Mutex<AnalysedTypeResolve>>,
}

impl SharedAnalysedTypeResolve {
    pub fn new(resolve: AnalysedTypeResolve) -> Self {
        Self {
            resolve: Arc::new(Mutex::new(resolve)),
        }
    }

    pub fn analysed_type(&mut self, name: &TypeName) -> Result<AnalysedType, String> {
        self.resolve.lock().unwrap().analysed_type(name)
    }
}

use crate::type_parameter_parser::type_parameter;
use bincode::{Decode, Encode};
use combine::stream::position;
use combine::EasyParser;
use std::fmt;
use std::fmt::Display;

// The type parameter which can be part of instance creation or worker function call
#[derive(Debug, Hash, Clone, Eq, PartialEq, PartialOrd, Ord, Encode, Decode)]
pub enum TypeParameter {
    Interface(InterfaceName),
    PackageName(PackageName),
    FullyQualifiedInterface(FullyQualifiedInterfaceName),
}

impl TypeParameter {
    pub fn get_package_name(&self) -> Option<PackageName> {
        match self {
            TypeParameter::Interface(_) => None,
            TypeParameter::PackageName(package) => Some(package.clone()),
            TypeParameter::FullyQualifiedInterface(qualified) => {
                Some(qualified.package_name.clone())
            }
        }
    }

    pub fn get_interface_name(&self) -> Option<InterfaceName> {
        match self {
            TypeParameter::Interface(interface) => Some(interface.clone()),
            TypeParameter::PackageName(_) => None,
            TypeParameter::FullyQualifiedInterface(qualified) => {
                Some(qualified.interface_name.clone())
            }
        }
    }

    pub fn from_text(input: &str) -> Result<TypeParameter, String> {
        type_parameter()
            .easy_parse(position::Stream::new(input))
            .map(|t| t.0)
            .map_err(|err| format!("Invalid type parameter type {}", err))
    }
}

impl Display for TypeParameter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypeParameter::Interface(interface) => write!(f, "{}", interface),
            TypeParameter::PackageName(package) => write!(f, "{}", package),
            TypeParameter::FullyQualifiedInterface(qualified) => write!(f, "{}", qualified),
        }
    }
}

// foo@1.0.0
#[derive(Debug, Hash, Clone, Eq, PartialEq, PartialOrd, Ord, Encode, Decode)]
pub struct InterfaceName {
    pub name: String,
    pub version: Option<String>,
}

impl Display for InterfaceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)?;
        if let Some(version) = &self.version {
            write!(f, "@{}", version)?;
        }
        Ok(())
    }
}

// ns2:pkg2@1.0.0
#[derive(Debug, Hash, Clone, Eq, PartialEq, PartialOrd, Ord, Encode, Decode)]
pub struct PackageName {
    pub namespace: String,
    pub package_name: String,
    pub version: Option<String>,
}

impl Display for PackageName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.namespace, self.package_name)?;
        if let Some(version) = &self.version {
            write!(f, "@{}", version)?;
        }
        Ok(())
    }
}

// ns2:pkg2/foo@1.0.0
#[derive(Debug, Hash, Clone, Eq, PartialEq, PartialOrd, Ord, Encode, Decode)]
pub struct FullyQualifiedInterfaceName {
    pub package_name: PackageName,
    pub interface_name: InterfaceName,
}

impl Display for FullyQualifiedInterfaceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.package_name, self.interface_name)
    }
}

use std::fmt;
use std::fmt::Display;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeParameter {
    Interface(InterfaceName),
    PackageName(PackageName),
    FullyQualifiedInterface(FullyQualifiedInterfaceName)
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceName {
    pub name: String,
    pub version: Option<String>
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageName {
    pub namespace: String,
    pub package_name: String,
    pub version: Option<String>
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullyQualifiedInterfaceName {
    pub package_name: PackageName,
    pub interface_name: InterfaceName
}

impl Display for FullyQualifiedInterfaceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.package_name, self.interface_name)
    }
}

use crate::FunctionTypeRegistry;
use crate::type_parameter::InstanceType;

// InstanceType is more or less a subset of FunctionTypeRegistry
// FunctionTypeRegistry is a collection of all the functions across all the interfaces and packages
// in a component, where as an InstanceType is a collection of functions that are available to a
// particular instance of a component - meaning specific to a ns:pkg, or interface or if ambiguous,
// ns:pkg/interface. Further disambiguousion can be done with respect to version
pub struct InstanceModule {
    pub instance_type: InstanceType,
    pub registry: FunctionTypeRegistry,
}

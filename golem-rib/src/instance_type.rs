use crate::generic_type_parameter::GenericTypeParameter;
use crate::type_parameter::InstanceType;
use crate::FunctionTypeRegistry;

// InstanceType is more or less a subset of FunctionTypeRegistry
// FunctionTypeRegistry is a collection of all the functions across all the interfaces and packages
// in a component, where as an InstanceType is a collection of functions that are available to a
// particular instance of a component - meaning specific to a ns:pkg, or interface or if ambiguous,
// ns:pkg/interface. Further disambiguousion can be done with respect to version
pub struct InstanceModule {
    pub instance_type: InstanceType,
    pub registry: FunctionTypeRegistry,
}

pub enum Instance {
    Durable {
        worker_name: String,
        instance_type: InstanceType,
        registry: FunctionTypeRegistry,
    },

    Ephemeral {
        instance_type: InstanceType,
        registry: FunctionTypeRegistry,
    },
}

impl Instance {
    // From a generic type parameter, we can
    pub fn from_generic_type_parameter(
        generic_type_parameter: &GenericTypeParameter,
        component_metadata: &Vec<FunctionTypeRegistry>,
        worker_name: Option<String>,
    ) {
    }
}

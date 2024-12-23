use anyhow::anyhow;
use protox::prost_reflect::prost_types::FileDescriptorSet;
use tailcall_valid::{Valid, Validator};
use crate::wit_config::config::WitConfig;
use crate::wit_config::wit_types::WitType;
use crate::proto::handle_services::handle_services;
use crate::proto::handle_types::handle_types;

pub struct Proto(Vec<FileDescriptorSet>);

impl Proto {
    pub fn new<T: IntoIterator<Item=FileDescriptorSet>>(v: T) -> Self {
        Self(v.into_iter().collect())
    }
    pub fn to_config(&self) -> Valid<WitConfig, anyhow::Error, anyhow::Error> {
        Valid::succeed(WitConfig::default())
            .and_then(|config| handle_types(config, &self.0, "api:todos@1.0.0".to_string()))
            .and_then(|config| handle_services(config, &self.0))
    }
}

pub fn process_ty(name: &str) -> Valid<WitType, anyhow::Error, anyhow::Error> {
    if !name.starts_with('.') {
        return Valid::fail(anyhow!("Expected fully-qualified name for reference type but got {name}. This is a bug!"));
    }
    let name = &name[1..];
    if let Some((_package, name)) = name.rsplit_once('.') {
        Valid::succeed(WitType::FieldTy(name.to_string()))
    }else {
        Valid::succeed(WitType::FieldTy(name.to_string()))
    }
}

#[cfg(test)]
mod test {
    use tailcall_valid::Validator;
    use wit_parser::Resolve;
    use crate::proto::proto::Proto;

    #[test]
    fn test_nested() {
        let relative = format!("{}/src/proto/fixtures",env!("CARGO_MANIFEST_DIR"));
       let proto = protox::compile([format!("{}/address.proto", relative)], [relative]).unwrap();
        let proto = Proto::new([proto]);
        let config = proto.to_config().to_result().unwrap();

        let mut resolve = Resolve::new();
        assert!(resolve.push_str("foox.wit", &config.to_wit()).is_ok());

        insta::assert_snapshot!(config.to_wit());
    }
}
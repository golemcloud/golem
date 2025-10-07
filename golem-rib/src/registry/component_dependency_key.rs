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
use uuid::Uuid;

#[derive(Debug, Hash, PartialEq, Eq, Clone, Ord, PartialOrd)]
pub struct ComponentDependencyKey {
    pub component_name: String,
    pub component_id: Uuid,
    pub component_version: u64,
    pub root_package_name: Option<String>,
    pub root_package_version: Option<String>,
}

impl bincode::Encode for ComponentDependencyKey {
    fn encode<E: bincode::enc::Encoder>(
        &self,
        encoder: &mut E,
    ) -> Result<(), bincode::error::EncodeError> {
        use bincode::enc::write::Writer;

        encoder.writer().write(self.component_name.as_bytes())?;
        self.component_id.as_bytes().encode(encoder)?;
        self.component_version.encode(encoder)?;
        Option::<String>::encode(&self.root_package_name, encoder)?;
        Option::<String>::encode(&self.root_package_version, encoder)?;

        Ok(())
    }
}

impl<Context> bincode::Decode<Context> for ComponentDependencyKey {
    fn decode<D: bincode::de::Decoder>(
        decoder: &mut D,
    ) -> Result<Self, bincode::error::DecodeError> {
        use bincode::de::read::Reader;

        let component_name = String::decode(decoder)?;
        let mut bytes = [0u8; 16];
        decoder.reader().read(&mut bytes)?;
        let component_id = Uuid::from_bytes(bytes);
        let component_version = u64::decode(decoder)?;
        let root_package_name = Option::<String>::decode(decoder)?;
        let root_package_version = Option::<String>::decode(decoder)?;

        Ok(ComponentDependencyKey {
            component_name,
            component_version,
            component_id,
            root_package_name,
            root_package_version,
        })
    }
}

impl<'de, Context> bincode::BorrowDecode<'de, Context> for ComponentDependencyKey {
    fn borrow_decode<D: bincode::de::BorrowDecoder<'de, Context = Context>>(
        decoder: &mut D,
    ) -> Result<Self, bincode::error::DecodeError> {
        use bincode::de::read::Reader;

        let component_name = String::borrow_decode(decoder)?;
        let mut bytes = [0u8; 16];
        decoder.reader().read(&mut bytes)?;
        let component_id = Uuid::from_bytes(bytes);
        let component_version = u64::borrow_decode(decoder)?;
        let root_package_name = Option::<String>::borrow_decode(decoder)?;
        let root_package_version = Option::<String>::borrow_decode(decoder)?;

        Ok(ComponentDependencyKey {
            component_name,
            component_id,
            component_version,
            root_package_name,
            root_package_version,
        })
    }
}

impl Display for ComponentDependencyKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Component: {}, ID: {}, Root Package: {}@{}",
            self.component_name,
            self.component_id,
            self.root_package_name.as_deref().unwrap_or("unknown"),
            self.root_package_version.as_deref().unwrap_or("unknown")
        )
    }
}

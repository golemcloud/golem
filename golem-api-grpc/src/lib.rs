// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

pub mod proto {
    use uuid::Uuid;
    tonic::include_proto!("mod");

    pub const FILE_DESCRIPTOR_SET: &[u8] = tonic::include_file_descriptor_set!("services");

    impl From<Uuid> for golem::common::Uuid {
        fn from(value: Uuid) -> Self {
            let (high_bits, low_bits) = value.as_u64_pair();
            golem::common::Uuid {
                high_bits,
                low_bits,
            }
        }
    }

    impl From<golem::common::Uuid> for Uuid {
        fn from(value: golem::common::Uuid) -> Self {
            let high_bits = value.high_bits;
            let low_bits = value.low_bits;
            Uuid::from_u64_pair(high_bits, low_bits)
        }
    }

    #[cfg(test)]
    mod tests {
        use std::str::FromStr;

        use crate::proto::golem;

        #[test]
        fn test_uuid() {
            let template_id = uuid::Uuid::from_str("040eeaee-08fa-4273-83ea-bc26e10574c1").unwrap();
            let token = uuid::Uuid::from_str("5816ed13-4d6e-40d0-8391-f0eb75378476").unwrap();

            let template_id_proto: golem::common::Uuid = template_id.into();
            let token_proto: golem::common::Uuid = token.into();

            println!("template_id_proto: {:?}", template_id_proto);
            println!("token_proto: {:?}", token_proto);
        }
    }
}

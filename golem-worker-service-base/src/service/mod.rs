// Copyright 2024-2025 Golem Cloud
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

pub mod component;
pub mod gateway;
pub mod worker;

pub fn with_metadata<T, I, K, V>(request: T, metadata: I) -> tonic::Request<T>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<str>,
    V: AsRef<str>,
{
    let mut req = tonic::Request::new(request);
    let req_metadata = req.metadata_mut();

    for (key, value) in metadata {
        let key = tonic::metadata::MetadataKey::from_bytes(key.as_ref().as_bytes());
        let value = value.as_ref().parse();
        if let (Ok(key), Ok(value)) = (key, value) {
            req_metadata.insert(key, value);
        }
    }

    req
}

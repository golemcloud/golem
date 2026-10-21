// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use super::SerializableHttpMethod;

#[cfg(test)]
mod serializable_http_method_tests {
    use test_r::test;

    use super::*;

    #[test]
    fn test_serializable_method_to_http() {
        assert_eq!(
            ::http::Method::try_from(&SerializableHttpMethod::Get).unwrap(),
            ::http::Method::GET
        );
        assert_eq!(
            ::http::Method::try_from(&SerializableHttpMethod::Post).unwrap(),
            ::http::Method::POST
        );
        assert_eq!(
            ::http::Method::try_from(&SerializableHttpMethod::Other("PURGE".to_string())).unwrap(),
            ::http::Method::from_bytes(b"PURGE").unwrap()
        );
    }
}

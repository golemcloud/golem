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

use crate::custom_api::http_test_context::{test_context_internal, HttpTestContext};
use golem_test_framework::config::EnvBasedTestDependencies;
use pretty_assertions::assert_eq;
use serde_yaml::Value;
use test_r::test_dep;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(EnvBasedTestDependencies);

#[test_dep]
async fn test_context(deps: &EnvBasedTestDependencies) -> HttpTestContext {
    test_context_internal(deps, "http_rust", "http:rust")
        .await
        .unwrap()
}

const EXPECTED_OPENAPI_YAML: &str = r#"
components: {}
info:
  title: ""
  version: ""
openapi: 3.0.0
paths:
  /cors-agents/{agent_name}/inherited:
    get:
      parameters:
      - description: "Path parameter: agent_name"
        explode: false
        in: path
        name: agent_name
        required: true
        schema:
          type: string
        style: simple
      responses:
        "200":
          content:
            application/json:
              schema:
                properties:
                  ok:
                    type: boolean
                required:
                - ok
                type: object
          description: Response 200
  /cors-agents/{agent_name}/preflight-required:
    post:
      parameters:
      - description: "Path parameter: agent_name"
        explode: false
        in: path
        name: agent_name
        required: true
        schema:
          type: string
        style: simple
      requestBody:
        content:
          application/json:
            schema:
              properties:
                body:
                  properties:
                    name:
                      type: string
                  required:
                  - name
                  type: object
              required:
              - body
              type: object
        description: JSON body
        required: true
      responses:
        "200":
          content:
            application/json:
              schema:
                properties:
                  received:
                    type: string
                required:
                - received
                type: object
          description: Response 200
  /cors-agents/{agent_name}/wildcard:
    get:
      parameters:
      - description: "Path parameter: agent_name"
        explode: false
        in: path
        name: agent_name
        required: true
        schema:
          type: string
        style: simple
      responses:
        "200":
          content:
            application/json:
              schema:
                properties:
                  ok:
                    type: boolean
                required:
                - ok
                type: object
          description: Response 200
  /http-agents/{agent_name}/json-body/{id}:
    post:
      parameters:
      - description: "Path parameter: agent_name"
        explode: false
        in: path
        name: agent_name
        required: true
        schema:
          type: string
        style: simple
      - description: "Path parameter: id"
        explode: false
        in: path
        name: id
        required: true
        schema:
          type: string
        style: simple
      requestBody:
        content:
          application/json:
            schema:
              properties:
                count:
                  format: int64
                  minimum: 0
                  type: integer
                name:
                  type: string
              required:
              - name
              - count
              type: object
        description: JSON body
        required: true
      responses:
        "200":
          content:
            application/json:
              schema:
                properties:
                  ok:
                    type: boolean
                required:
                - ok
                type: object
          description: Response 200
  /http-agents/{agent_name}/multi-path-vars/{first}/{second}:
    get:
      parameters:
      - description: "Path parameter: agent_name"
        explode: false
        in: path
        name: agent_name
        required: true
        schema:
          type: string
        style: simple
      - description: "Path parameter: first"
        explode: false
        in: path
        name: first
        required: true
        schema:
          type: string
        style: simple
      - description: "Path parameter: second"
        explode: false
        in: path
        name: second
        required: true
        schema:
          type: string
        style: simple
      responses:
        "200":
          content:
            application/json:
              schema:
                properties:
                  joined:
                    type: string
                required:
                - joined
                type: object
          description: Response 200
  /http-agents/{agent_name}/path-and-header/{resource_id}:
    get:
      parameters:
      - description: "Path parameter: agent_name"
        explode: false
        in: path
        name: agent_name
        required: true
        schema:
          type: string
        style: simple
      - description: "Path parameter: resource_id"
        explode: false
        in: path
        name: resource_id
        required: true
        schema:
          type: string
        style: simple
      - description: "Header parameter: x-request-id"
        explode: false
        in: header
        name: x-request-id
        required: true
        schema:
          type: string
        style: simple
      responses:
        "200":
          content:
            application/json:
              schema:
                properties:
                  request-id:
                    type: string
                  resource-id:
                    type: string
                required:
                - resource-id
                - request-id
                type: object
          description: Response 200
  /http-agents/{agent_name}/path-and-query/{item_id}:
    get:
      parameters:
      - description: "Path parameter: agent_name"
        explode: false
        in: path
        name: agent_name
        required: true
        schema:
          type: string
        style: simple
      - description: "Path parameter: item_id"
        explode: false
        in: path
        name: item_id
        required: true
        schema:
          type: string
        style: simple
      - allowEmptyValue: false
        description: "Query parameter: limit"
        explode: false
        in: query
        name: limit
        required: true
        schema:
          format: int64
          minimum: 0
          type: integer
        style: form
      responses:
        "200":
          content:
            application/json:
              schema:
                properties:
                  id:
                    type: string
                  limit:
                    format: int64
                    minimum: 0
                    type: integer
                required:
                - id
                - limit
                type: object
          description: Response 200
  /http-agents/{agent_name}/resp/binary:
    get:
      parameters:
      - description: "Path parameter: agent_name"
        explode: false
        in: path
        name: agent_name
        required: true
        schema:
          type: string
        style: simple
      responses:
        "200":
          content:
            application/octet-stream:
              schema:
                format: binary
                type: string
          description: Response 200
  /http-agents/{agent_name}/resp/json:
    get:
      parameters:
      - description: "Path parameter: agent_name"
        explode: false
        in: path
        name: agent_name
        required: true
        schema:
          type: string
        style: simple
      responses:
        "200":
          content:
            application/json:
              schema:
                properties:
                  value:
                    type: string
                required:
                - value
                type: object
          description: Response 200
  /http-agents/{agent_name}/resp/no-content:
    get:
      parameters:
      - description: "Path parameter: agent_name"
        explode: false
        in: path
        name: agent_name
        required: true
        schema:
          type: string
        style: simple
      responses:
        "204":
          description: Response 204
  /http-agents/{agent_name}/resp/optional/{found}:
    get:
      parameters:
      - description: "Path parameter: agent_name"
        explode: false
        in: path
        name: agent_name
        required: true
        schema:
          type: string
        style: simple
      - description: "Path parameter: found"
        explode: false
        in: path
        name: found
        required: true
        schema:
          type: boolean
        style: simple
      responses:
        "200":
          content:
            application/json:
              schema:
                properties:
                  value:
                    type: string
                required:
                - value
                type: object
          description: Response 200
        "404":
          description: Response 404
  /http-agents/{agent_name}/resp/result-json-json/{ok}:
    get:
      parameters:
      - description: "Path parameter: agent_name"
        explode: false
        in: path
        name: agent_name
        required: true
        schema:
          type: string
        style: simple
      - description: "Path parameter: ok"
        explode: false
        in: path
        name: ok
        required: true
        schema:
          type: boolean
        style: simple
      responses:
        "200":
          content:
            application/json:
              schema:
                properties:
                  value:
                    type: string
                required:
                - value
                type: object
          description: Response 200
        "500":
          description: Response 500
  /http-agents/{agent_name}/resp/result-json-void:
    get:
      parameters:
      - description: "Path parameter: agent_name"
        explode: false
        in: path
        name: agent_name
        required: true
        schema:
          type: string
        style: simple
      responses:
        "200":
          content:
            application/json:
              schema:
                properties:
                  value:
                    type: string
                required:
                - value
                type: object
          description: Response 200
        "500":
          content:
            "*/*": {}
          description: Response 500
  /http-agents/{agent_name}/resp/result-void-json:
    post:
      parameters:
      - description: "Path parameter: agent_name"
        explode: false
        in: path
        name: agent_name
        required: true
        schema:
          type: string
        style: simple
      responses:
        "200":
          content:
            "*/*": {}
          description: Response 200
        "500":
          description: Response 500
  /http-agents/{agent_name}/rest/{tail}:
    get:
      parameters:
      - description: "Path parameter: agent_name"
        explode: false
        in: path
        name: agent_name
        required: true
        schema:
          type: string
        style: simple
      - description: "Path parameter: tail"
        explode: false
        in: path
        name: tail
        required: true
        schema:
          type: string
        style: simple
      responses:
        "200":
          content:
            application/json:
              schema:
                properties:
                  tail:
                    type: string
                required:
                - tail
                type: object
          description: Response 200
  /http-agents/{agent_name}/restricted-unstructured-binary/{bucket}:
    post:
      parameters:
      - description: "Path parameter: agent_name"
        explode: false
        in: path
        name: agent_name
        required: true
        schema:
          type: string
        style: simple
      - description: "Path parameter: bucket"
        explode: false
        in: path
        name: bucket
        required: true
        schema:
          type: string
        style: simple
      requestBody:
        content:
          image/gif: {}
        description: Restricted binary body
        required: true
      responses:
        "200":
          content:
            application/json:
              schema:
                format: int64
                type: integer
          description: Response 200
  /http-agents/{agent_name}/string-path-var/{path_var}:
    get:
      parameters:
      - description: "Path parameter: agent_name"
        explode: false
        in: path
        name: agent_name
        required: true
        schema:
          type: string
        style: simple
      - description: "Path parameter: path_var"
        explode: false
        in: path
        name: path_var
        required: true
        schema:
          type: string
        style: simple
      responses:
        "200":
          content:
            application/json:
              schema:
                properties:
                  value:
                    type: string
                required:
                - value
                type: object
          description: Response 200
  /http-agents/{agent_name}/unrestricted-unstructured-binary/{bucket}:
    post:
      parameters:
      - description: "Path parameter: agent_name"
        explode: false
        in: path
        name: agent_name
        required: true
        schema:
          type: string
        style: simple
      - description: "Path parameter: bucket"
        explode: false
        in: path
        name: bucket
        required: true
        schema:
          type: string
        style: simple
      requestBody:
        content:
          "*/*": {}
        description: Unrestricted binary body
        required: true
      responses:
        "200":
          content:
            application/json:
              schema:
                format: int64
                type: integer
          description: Response 200
  /webhook-agents/{agent_name}/set-test-server-url:
    post:
      parameters:
      - description: "Path parameter: agent_name"
        explode: false
        in: path
        name: agent_name
        required: true
        schema:
          type: string
        style: simple
      requestBody:
        content:
          application/json:
            schema:
              properties:
                test-server-url:
                  type: string
              required:
              - test-server-url
              type: object
        description: JSON body
        required: true
      responses:
        "204":
          description: Response 204
  /webhook-agents/{agent_name}/test-webhook:
    post:
      parameters:
      - description: "Path parameter: agent_name"
        explode: false
        in: path
        name: agent_name
        required: true
        schema:
          type: string
        style: simple
      responses:
        "200":
          content:
            application/json:
              schema:
                properties:
                  payload-length:
                    format: int64
                    minimum: 0
                    type: integer
                required:
                - payload-length
                type: object
          description: Response 200"#;

#[test]
#[tracing::instrument]
async fn test_open_api_generation(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(agent.base_url.join("/openapi.json")?)
        .header("Origin", "https://mount.example.com")
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let bytes = response.bytes().await?;
    let actual: serde_yaml::Value = serde_json::from_slice(&bytes)?;
    let expected: Value = serde_yaml::from_str(EXPECTED_OPENAPI_YAML).unwrap();

    assert_eq!(actual, expected);

    Ok(())
}

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

use assert2::assert;
use axum::http::{HeaderMap, HeaderValue};
use golem_client::api::RegistryServiceClient;
use golem_client::model::DeploymentCreation;
use golem_common::model::component::ComponentName;
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::domain_registration::{Domain, DomainRegistrationCreation};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::http_api_definition::{
    GatewayBinding, HttpApiDefinitionCreation, HttpApiDefinitionName, HttpApiDefinitionVersion,
    HttpApiRoute, RouteMethod, WorkerGatewayBinding,
};
use golem_common::model::http_api_deployment::HttpApiDeploymentCreation;
use golem_common::model::Empty;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use reqwest::Url;
use serde_json::json;
use std::fmt::{Debug, Formatter};
use test_r::test_dep;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(EnvBasedTestDependencies);

pub struct ShoppingCart {
    pub user: TestUserContext<EnvBasedTestDependencies>,
    pub env_id: EnvironmentId,
    pub deployment_revision: DeploymentRevision,
    pub client: reqwest::Client,
    pub base_url: Url,
}

impl Debug for ShoppingCart {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "ShoppingCart")
    }
}

#[test_dep]
async fn shopping_cart(deps: &EnvBasedTestDependencies) -> ShoppingCart {
    shopping_cart_internal(deps).await.unwrap()
}

async fn shopping_cart_internal(deps: &EnvBasedTestDependencies) -> anyhow::Result<ShoppingCart> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user.app_and_env().await?;

    // needs to be static as it's used for hash calculation
    let domain = Domain(format!("{}.golem.cloud", env.id));

    client
        .create_domain_registration(
            &env.id.0,
            &DomainRegistrationCreation {
                domain: domain.clone(),
            },
        )
        .await?;

    user.component(&env.id, "shopping-cart").store().await?;

    let http_api_definition_creation = HttpApiDefinitionCreation {
        name: HttpApiDefinitionName("test-api".to_string()),
        version: HttpApiDefinitionVersion("1".to_string()),
        routes: vec![
            HttpApiRoute {
                method: RouteMethod::Post,
                path: "/{user-id}/contents".to_string(),
                binding: GatewayBinding::Worker(WorkerGatewayBinding {
                    component_name: ComponentName("shopping-cart".to_string()),
                    idempotency_key: None,
                    invocation_context: None,
                    response: r#"
                        let user-id = request.path.user-id;
                        let worker = "shopping-cart-${user-id}";
                        let inst = instance(worker);
                        inst.add-item({
                            product-id: request.body.product-id,
                            name: request.body.name,
                            price: request.body.price,
                            quantity: request.body.quantity
                        });
                        {
                            status: 204
                        }
                    "#
                    .to_string(),
                }),
                security: None,
            },
            HttpApiRoute {
                method: RouteMethod::Get,
                path: "/{user-id}/contents".to_string(),
                binding: GatewayBinding::Worker(WorkerGatewayBinding {
                    component_name: ComponentName("shopping-cart".to_string()),
                    idempotency_key: None,
                    invocation_context: None,
                    response: r#"
                        let user-id = request.path.user-id;
                        let worker = "shopping-cart-${user-id}";
                        let inst = instance(worker);
                        let contents = inst.get-cart-contents();
                        {
                            body: contents,
                            status: 200
                        }
                    "#
                    .to_string(),
                }),
                security: None,
            },
            HttpApiRoute {
                method: RouteMethod::Get,
                path: "/swagger-ui".to_string(),
                binding: GatewayBinding::SwaggerUi(Empty {}),
                security: None,
            },
        ],
    };

    client
        .create_http_api_definition(&env.id.0, &http_api_definition_creation)
        .await?;

    let http_api_deployment_creation = HttpApiDeploymentCreation {
        domain: domain.clone(),
        api_definitions: vec![HttpApiDefinitionName("test-api".to_string())],
    };

    client
        .create_http_api_deployment(&env.id.0, &http_api_deployment_creation)
        .await?;

    let plan = client.get_environment_deployment_plan(&env.id.0).await?;

    let deployment = client
        .deploy_environment(
            &env.id.0,
            &DeploymentCreation {
                current_deployment_revision: None,
                expected_deployment_hash: plan.deployment_hash,
                version: "0.0.1".to_string(),
            },
        )
        .await?;

    let client = {
        let mut headers = HeaderMap::new();
        headers.insert("Host", HeaderValue::from_str(&domain.0)?);
        reqwest::Client::builder()
            .default_headers(headers)
            .build()?
    };

    let base_url = Url::parse(&format!("http://127.0.0.1:{}", user.custom_request_port()))?;

    Ok(ShoppingCart {
        client,
        base_url,
        user,
        env_id: env.id,
        deployment_revision: deployment.revision,
    })
}

#[test]
#[tracing::instrument]
async fn request_to_wrong_domain_results_in_404(cart: &ShoppingCart) -> anyhow::Result<()> {
    // use fresh client so we don't send default host header
    let client = reqwest::Client::new();
    let response = client
        .get(cart.base_url.join("/1/contents")?)
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn get_shopping_cart_contents(cart: &ShoppingCart) -> anyhow::Result<()> {
    let response = cart
        .client
        .get(cart.base_url.join("/2/contents")?)
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await?;

    assert!(body == json!([]));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn add_and_get_shopping_cart_contents(cart: &ShoppingCart) -> anyhow::Result<()> {
    let item = json!({
        "product-id": "1",
        "name": "golem-plush",
        "price": 15.0,
        "quantity": 1
    });

    {
        let response = cart
            .client
            .post(cart.base_url.join("/3/contents")?)
            .json(&item)
            .send()
            .await?;
        assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
    }

    {
        let response = cart
            .client
            .get(cart.base_url.join("/3/contents")?)
            .send()
            .await?;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body: serde_json::Value = response.json().await?;
        assert!(body == json!([item]));
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn swagger_ui(cart: &ShoppingCart) -> anyhow::Result<()> {
    let response = cart
        .client
        .get(cart.base_url.join("swagger-ui")?)
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn open_api_spec(cart: &ShoppingCart) -> anyhow::Result<()> {
    let client = cart.user.registry_service_client().await;
    let spec = client
        .get_openapi_of_http_api_definition_in_deployment(
            &cart.env_id.0,
            cart.deployment_revision.0,
            "test-api",
        )
        .await?;

    let expected = json!(
        {
          "components": {},
          "info": {
            "title": "test-api",
            "version": "1"
          },
          "openapi": "3.0.0",
          "paths": {
            "/swagger-ui": {
              "get": {
                "responses": {
                  "200": {
                    "content": {
                      "text/html": {
                        "schema": {
                          "type": "string"
                        }
                      }
                    },
                    "description": "Response"
                  }
                }
              }
            },
            "/{user-id}/contents": {
              "get": {
                "parameters": [
                  {
                    "description": "Path parameter: user-id",
                    "explode": false,
                    "in": "path",
                    "name": "user-id",
                    "required": true,
                    "schema": {
                      "type": "string"
                    },
                    "style": "simple"
                  }
                ],
                "responses": {
                  "default": {
                    "content": {
                      "application/json": {
                        "schema": {
                          "items": {
                            "properties": {
                              "name": {
                                "type": "string"
                              },
                              "price": {
                                "format": "float",
                                "type": "number"
                              },
                              "product-id": {
                                "type": "string"
                              },
                              "quantity": {
                                "format": "int32",
                                "minimum": 0,
                                "type": "integer"
                              }
                            },
                            "required": [
                              "product-id",
                              "name",
                              "price",
                              "quantity"
                            ],
                            "type": "object"
                          },
                          "type": "array"
                        }
                      }
                    },
                    "description": "Response"
                  }
                }
              },
              "post": {
                "parameters": [
                  {
                    "description": "Path parameter: user-id",
                    "explode": false,
                    "in": "path",
                    "name": "user-id",
                    "required": true,
                    "schema": {
                      "type": "string"
                    },
                    "style": "simple"
                  }
                ],
                "requestBody": {
                  "content": {
                    "application/json": {
                      "schema": {
                        "properties": {
                          "name": {
                            "type": "string"
                          },
                          "price": {
                            "format": "float",
                            "type": "number"
                          },
                          "product-id": {
                            "type": "string"
                          },
                          "quantity": {
                            "format": "int32",
                            "minimum": 0,
                            "type": "integer"
                          }
                        },
                        "required": [
                          "name",
                          "price",
                          "product-id",
                          "quantity"
                        ],
                        "type": "object"
                      }
                    }
                  },
                  "description": "Request payload",
                  "required": true
                },
                "responses": {
                  "default": {
                    "description": "Response"
                  }
                }
              }
            }
          }
        }
    );

    assert!(spec.0 == expected);
    Ok(())
}

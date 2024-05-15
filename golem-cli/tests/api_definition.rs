use crate::cli::{Cli, CliLive};
use crate::worker::make_component_from_file;
use golem_cli::model::component::ComponentView;
use golem_client::model::{GolemWorkerBinding, HttpApiDefinition, MethodPattern, Route};
use golem_test_framework::config::TestDependencies;
use libtest_mimic::{Failed, Trial};
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

fn make(
    suffix: &str,
    name: &str,
    cli: CliLive,
    deps: Arc<dyn TestDependencies + Send + Sync + 'static>,
) -> Vec<Trial> {
    let ctx = (deps, name.to_string(), cli);
    vec![
        Trial::test_in_context(
            format!("api_definition_import{suffix}"),
            ctx.clone(),
            api_definition_import,
        ),
        Trial::test_in_context(
            format!("api_definition_add{suffix}"),
            ctx.clone(),
            api_definition_add,
        ),
        Trial::test_in_context(
            format!("api_definition_update{suffix}"),
            ctx.clone(),
            api_definition_update,
        ),
        Trial::test_in_context(
            format!("api_definition_update_immutable{suffix}"),
            ctx.clone(),
            api_definition_update_immutable,
        ),
        Trial::test_in_context(
            format!("api_definition_list{suffix}"),
            ctx.clone(),
            api_definition_list,
        ),
        Trial::test_in_context(
            format!("api_definition_list_versions{suffix}"),
            ctx.clone(),
            api_definition_list_versions,
        ),
        Trial::test_in_context(
            format!("api_definition_get{suffix}"),
            ctx.clone(),
            api_definition_get,
        ),
        Trial::test_in_context(
            format!("api_definition_delete{suffix}"),
            ctx.clone(),
            api_definition_delete,
        ),
    ]
}

pub fn all(deps: Arc<dyn TestDependencies + Send + Sync + 'static>) -> Vec<Trial> {
    let mut short_args = make(
        "_short",
        "CLI_short",
        CliLive::make(deps.clone()).unwrap().with_short_args(),
        deps.clone(),
    );

    let mut long_args = make(
        "_long",
        "CLI_long",
        CliLive::make(deps.clone()).unwrap().with_long_args(),
        deps,
    );

    short_args.append(&mut long_args);

    short_args
}

pub fn make_shopping_cart_component(
    deps: Arc<dyn TestDependencies + Send + Sync + 'static>,
    component_name: &str,
    cli: &CliLive,
) -> Result<ComponentView, Failed> {
    make_component_from_file(deps, component_name, cli, "shopping-cart.wasm")
}

fn make_file(id: &str, json: &serde_json::value::Value) -> Result<PathBuf, Failed> {
    let text = serde_json::to_string_pretty(json)?;

    let path = PathBuf::from(format!("../target/api-definition-{id}.json"));

    fs::write(&path, text)?;

    Ok(path)
}

fn golem_def_with_response(
    id: &str,
    component_id: &str,
    response: Option<String>,
) -> HttpApiDefinition {
    HttpApiDefinition {
        id: id.to_string(),
        version: "0.1.0".to_string(),
        draft: true,
        routes: vec![Route {
            method: MethodPattern::Get,
            path: "/{user-id}/get-cart-contents".to_string(),
            binding: GolemWorkerBinding {
                component_id: Uuid::parse_str(component_id).unwrap(),
                worker_name: "worker-${request.path.user-id}".to_string(),
                function_name: "golem:it/api/get-cart-contents".to_string(),
                function_params: vec![],
                idempotency_key: None,
                response,
            },
        }],
    }
}

pub fn golem_def(id: &str, component_id: &str) -> HttpApiDefinition {
    golem_def_with_response(id, component_id, Some("${{headers: {ContentType: 'json', userid: 'foo'}, body: worker.response, status: 200}}".to_string()))
}

pub fn make_golem_file(def: &HttpApiDefinition) -> Result<PathBuf, Failed> {
    let golem_json = serde_json::to_value(def)?;

    make_file(&def.id, &golem_json)
}

pub fn make_open_api_file(id: &str, component_id: &str) -> Result<PathBuf, Failed> {
    let open_api_json = json!(
      {
        "openapi": "3.0.0",
        "info": {
          "title": "Sample API",
          "version": "1.0.2"
        },
        "x-golem-api-definition-id": id,
        "x-golem-api-definition-version": "0.1.0",
        "paths": {
            "/{user-id}/get-cart-contents": {
              "x-golem-worker-bridge": {
                "worker-id": "worker-${request.path.user-id}",
                "function-name": "golem:it/api/get-cart-contents",
                "function-params": [],
                "component-id": component_id,
                "response" : "${{headers : {ContentType: 'json', userid: 'foo'}, body: worker.response, status: 200}}"
              },
              "get": {
                "summary": "Get Cart Contents",
                "description": "Get the contents of a user's cart",
                "parameters": [
                  {
                    "name": "user-id",
                    "in": "path",
                    "required": true,
                    "schema": {
                      "type": "string"
                    }
                  }
                ],
                "responses": {
                  "200": {
                    "description": "OK",
                    "content":{
                      "application/json": {
                        "schema": {
                          "$ref": "#/components/schemas/CartItem"
                        }
                      }

                    }
                  },
                  "404": {
                    "description": "Contents not found"
                  }
                }
              }
            }
          },
          "components": {
            "schemas": {
              "CartItem": {
                "type": "object",
                "properties": {
                  "id": {
                    "type": "string"
                  },
                  "name": {
                    "type": "string"
                  },
                  "price": {
                    "type": "number"
                  }
                }
              }
            }
        }
        }
    );

    make_file(id, &open_api_json)
}

fn api_definition_import(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_name = format!("api_definition_import{name}");
    let component = make_shopping_cart_component(deps, &component_name, &cli)?;
    let path = make_open_api_file(&component_name, &component.component_id)?;

    let res: HttpApiDefinition = cli.run(&["api-definition", "import", path.to_str().unwrap()])?;

    let expected = golem_def(&component_name, &component.component_id);

    assert_eq!(res, expected);

    Ok(())
}

fn api_definition_add(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_name = format!("api_definition_add{name}");
    let component = make_shopping_cart_component(deps, &component_name, &cli)?;
    let def = golem_def(&component_name, &component.component_id);
    let path = make_golem_file(&def)?;

    let res: HttpApiDefinition = cli.run(&["api-definition", "add", path.to_str().unwrap()])?;

    assert_eq!(res, def);

    Ok(())
}

fn api_definition_update(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_name = format!("api_definition_update{name}");
    let component = make_shopping_cart_component(deps, &component_name, &cli)?;

    let def = golem_def(&component_name, &component.component_id);
    let path = make_golem_file(&def)?;
    let _: HttpApiDefinition = cli.run(&["api-definition", "add", path.to_str().unwrap()])?;

    let updated = golem_def_with_response(&component_name, &component.component_id, Some("${{headers: {ContentType: 'json', userid: 'bar'}, body: worker.response, status: 200}}".to_string()));
    let path = make_golem_file(&updated)?;
    let res: HttpApiDefinition = cli.run(&["api-definition", "update", path.to_str().unwrap()])?;

    assert_eq!(res, updated);

    Ok(())
}

fn api_definition_update_immutable(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_name = format!("api_definition_update_immutable{name}");
    let component = make_shopping_cart_component(deps, &component_name, &cli)?;

    let mut def = golem_def(&component_name, &component.component_id);
    def.draft = false;
    let path = make_golem_file(&def)?;
    let _: HttpApiDefinition = cli.run(&["api-definition", "add", path.to_str().unwrap()])?;

    let updated = golem_def_with_response(&component_name, &component.component_id, Some("${{headers: {ContentType: 'json', userid: 'bar'}, body: worker.response, status: 200}}".to_string()));
    let path = make_golem_file(&updated)?;
    let res = cli.run_string(&["api-definition", "update", path.to_str().unwrap()]);

    assert!(res.is_err());

    Ok(())
}

fn api_definition_list(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_name = format!("api_definition_list{name}");
    let component = make_shopping_cart_component(deps, &component_name, &cli)?;
    let def = golem_def(&component_name, &component.component_id);
    let path = make_golem_file(&def)?;

    let _: HttpApiDefinition = cli.run(&["api-definition", "add", path.to_str().unwrap()])?;

    let res: Vec<HttpApiDefinition> = cli.run(&["api-definition", "list"])?;

    assert!(res.contains(&def));

    Ok(())
}

fn api_definition_list_versions(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_name = format!("api_definition_list_versions{name}");
    let component = make_shopping_cart_component(deps, &component_name, &cli)?;
    let def = golem_def(&component_name, &component.component_id);
    let path = make_golem_file(&def)?;
    let cfg = &cli.config;

    let _: HttpApiDefinition = cli.run(&["api-definition", "add", path.to_str().unwrap()])?;

    let res: Vec<HttpApiDefinition> = cli.run(&[
        "api-definition",
        "list",
        &cfg.arg('i', "id"),
        &component_name,
    ])?;

    assert_eq!(res.len(), 1);
    assert_eq!(*res.first().unwrap(), def);

    Ok(())
}

fn api_definition_get(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_name = format!("api_definition_get{name}");
    let component = make_shopping_cart_component(deps, &component_name, &cli)?;
    let def = golem_def(&component_name, &component.component_id);
    let path = make_golem_file(&def)?;

    let _: HttpApiDefinition = cli.run(&["api-definition", "add", path.to_str().unwrap()])?;

    let cfg = &cli.config;

    let res: HttpApiDefinition = cli.run(&[
        "api-definition",
        "get",
        &cfg.arg('i', "id"),
        &component_name,
        &cfg.arg('V', "version"),
        "0.1.0",
    ])?;

    assert_eq!(res, def);

    Ok(())
}

fn api_definition_delete(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_name = format!("api_definition_delete{name}");
    let component = make_shopping_cart_component(deps, &component_name, &cli)?;
    let def = golem_def(&component_name, &component.component_id);
    let path = make_golem_file(&def)?;

    let _: HttpApiDefinition = cli.run(&["api-definition", "add", path.to_str().unwrap()])?;

    let cfg = &cli.config;

    let res: HttpApiDefinition = cli.run(&[
        "api-definition",
        "get",
        &cfg.arg('i', "id"),
        &component_name,
        &cfg.arg('V', "version"),
        "0.1.0",
    ])?;

    assert_eq!(res, def);

    cli.run_unit(&[
        "api-definition",
        "delete",
        &cfg.arg('i', "id"),
        &component_name,
        &cfg.arg('V', "version"),
        "0.1.0",
    ])?;

    let res_list: Vec<HttpApiDefinition> = cli.run(&[
        "api-definition",
        "list",
        &cfg.arg('i', "id"),
        &component_name,
    ])?;

    assert!(res_list.is_empty());

    Ok(())
}

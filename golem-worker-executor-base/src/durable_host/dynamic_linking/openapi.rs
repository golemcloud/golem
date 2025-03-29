// OpenApiReqBuilder

use golem_common::model::component_metadata::OpenApiMetadata;
use http::Method;
use regex::Regex;
use reqwest::Client;
use rib::ParsedFunctionName;
use serde_json::Value as JsonValue;
use std::str::FromStr;
use url::Url;

pub struct OpenApiClient {
    function_name: ParsedFunctionName,
    params_json_values: Vec<JsonValue>,
    constructor_params: Vec<JsonValue>,
    metadata: Option<OpenApiMetadata>,
}
impl OpenApiClient {
    pub fn new(
        function_name: &ParsedFunctionName,
        params_json_values: Vec<JsonValue>,
        constructor_params: Vec<JsonValue>,
        metadata: Option<OpenApiMetadata>,
    ) -> Self {
        Self {
            function_name: function_name.clone(),
            params_json_values,
            constructor_params,
            metadata,
        }
    }

    pub async fn execute(&self) -> anyhow::Result<Vec<JsonValue>> {
        // 1. checks
        // 2. get no of path params, number of query params
        // 3. get query params names
        // 3. check the /endpoint from metadata, use 2, 3 to fill the path params and query params
        // 4. get the method from metadata
        // 5. pass last param as body to the req
        // 6. parse the result json to JsonValue
        // use reqwest

        // avoid first 1 parameters param1 is a resource Handle
        let _handle = self.params_json_values[0].clone();

        let configuration = self.constructor_params[0].clone();

        let metadata = self.metadata.as_ref().unwrap();

        let Some(endpoint_config) = metadata.endpoints.get(&self.function_name.to_string()) else {
            panic!("function name not found");
        };

        let method = &endpoint_config.method;
        let no_of_path_params = endpoint_config.no_path_params;
        let no_of_query_params = endpoint_config.no_query_params;
        let query_params_names = endpoint_config.query_param_names.as_ref().unwrap();

        let has_body = endpoint_config.has_body;

        let url = configuration.get("base_url").unwrap().as_str().unwrap();

        let client = Client::new();

        if no_of_path_params > 0 {
            Self::fill_path_params(
                url,
                &self.params_json_values[1..(no_of_path_params + 1) as usize].into(),
            );
        }
        // Prepare query parameters
        let mut url = Url::parse(url)?;

        if no_of_query_params > 0 {
            Self::fill_query_params(
                &mut url,
                self.params_json_values[(no_of_path_params + 1) as usize
                    ..(no_of_query_params + no_of_path_params + 1) as usize]
                    .into(),
                query_params_names,
            );
        }

        // // Convert the URL into a Uri type
        // let uri: Uri = url.as_str().parse()?;

        let req_builder = client.request(Method::from_str(method.as_str()).unwrap(), url);

        let req_builder = if has_body {
            req_builder
                .json(&self.params_json_values[self.params_json_values.len() - 1].to_string())
        } else {
            req_builder
        };

        let res = req_builder.send().await?;
        let body = res.text().await?;
        let json_value: JsonValue = serde_json::from_str(&body)?;
        Ok(vec![json_value])
    }

    pub async fn execute_async(&self) -> anyhow::Result<Vec<JsonValue>> {
        // 1. checks
        // 2. get no of path params, number of query params
        // 3. get query params names
        // 3. check the /endpoint from metadata, use 2, 3 to fill the path params and query params
        // 4. get the method from metadata
        // 5. pass last param as body to the req
        // 6. parse the result json to JsonValue
        // use reqwest

        // avoid first 1 parameters param1 is a resource Handle
        let _handle = self.params_json_values[0].clone();

        let configuration = self.constructor_params[0].clone();

        let metadata = self.metadata.as_ref().unwrap();

        let Some(endpoint_config) = metadata.endpoints.get(&self.function_name.to_string()) else {
            panic!("function name not found");
        };

        let method = &endpoint_config.method;
        let no_of_path_params = endpoint_config.no_path_params;
        let no_of_query_params = endpoint_config.no_query_params;
        let query_params_names = endpoint_config.query_param_names.as_ref().unwrap();

        let has_body = endpoint_config.has_body;

        let url = configuration.get("base_url").unwrap().as_str().unwrap();

        let client = Client::new();

        if no_of_path_params > 0 {
            Self::fill_path_params(
                url,
                &self.params_json_values[1..(no_of_path_params + 1) as usize].into(),
            );
        }
        // Prepare query parameters
        let mut url = Url::parse(url)?;

        if no_of_query_params > 0 {
            Self::fill_query_params(
                &mut url,
                self.params_json_values[(no_of_path_params + 1) as usize
                    ..(no_of_query_params + no_of_path_params + 1) as usize]
                    .into(),
                query_params_names,
            );
        }

        // // Convert the URL into a Uri type
        // let uri: Uri = url.as_str().parse()?;

        let req_builder = client.request(Method::from_str(method.as_str()).unwrap(), url);

        let req_builder = if has_body {
            req_builder
                .json(&self.params_json_values[self.params_json_values.len() - 1].to_string())
        } else {
            req_builder
        };

        let res = req_builder.send().await?;
        let body = res.text().await?;
        let json_value: JsonValue = serde_json::from_str(&body)?;
        Ok(vec![json_value])
    }

    fn fill_path_params(url: &str, path_params: &Vec<JsonValue>) -> String {
        let re = Regex::new(r"\{(\w+)\}").unwrap();
        let mut replaced_url = url.to_string();

        for param in path_params {
            if let Some(param_) = param.as_str() {
                replaced_url = re.replace(&replaced_url, param_).to_string();
            } else {
                panic!("path param is none");
            };
        }
        replaced_url
    }

    fn fill_query_params(
        url: &mut Url,
        query_params: Vec<JsonValue>,
        query_param_names: &Vec<String>,
    ) {
        // Construct the query string
        if !query_params.is_empty() {
            let query_str: String = query_params
                .iter()
                .zip(query_param_names)
                .map(|param| format!("{}={}", param.1, param.0))
                .collect::<Vec<String>>()
                .join("&");
            url.set_query(Some(&query_str));
        }
    }
}

use std::collections::HashMap;
use std::fmt::Display;

use crate::expression::Expr;
use crate::worker_binding::ResponseMapping;

#[derive(PartialEq, Debug, Clone)]
pub struct HttpResponseMapping {
    pub body: Expr,   // ${function.return}
    pub status: Expr, // "200" or if ${response.body.id == 1} "200" else "400"
    pub headers: HashMap<String, Expr>,
}

impl Display for HttpResponseMapping {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let response_mapping: ResponseMapping = self.clone().into();
        let expr_json = Expr::to_json_value(&response_mapping.0).unwrap();

        write!(f, "{}", expr_json.to_string())
    }
}

impl Into<ResponseMapping> for HttpResponseMapping {
    fn into(self) -> ResponseMapping {
        let mut headers = vec![];
        for (key, value) in self.headers {
            headers.push((key, Box::new(value)));
        }

        ResponseMapping(Expr::Record(
            vec![
                ("body".to_string(), Box::new(self.body)),
                ("status".to_string(), Box::new(self.status)),
                ("headers".to_string(), Box::new(Expr::Record(headers))),
            ]
            .into_iter()
            .collect(),
        ))
    }
}

impl TryFrom<&ResponseMapping> for HttpResponseMapping {
    type Error = String;

    fn try_from(response_mapping: &ResponseMapping) -> Result<Self, Self::Error> {
        let mut headers = HashMap::new();
        let generic_expr = &response_mapping.0;

        match generic_expr {
            Expr::Record(obj) => {
                let mut body = None;
                let mut status = None;

                for (key, value) in obj {
                    match key.as_str() {
                        "body" => body = Some(value),
                        "status" => status = Some(value),
                        "headers" => {
                            if let Expr::Record(headers_obj) = value.as_ref().clone() {
                                for (header_key, header_value) in headers_obj {
                                    headers
                                        .insert(header_key.clone(), header_value.as_ref().clone());
                                }
                            } else {
                                return Err("headers must be an object".to_string());
                            }
                        }
                        _ => return Err(format!("Unknown key: {}", key)),
                    }
                }

                match (body, status) {
                    (Some(body), Some(status)) => Ok(HttpResponseMapping {
                        body: body.as_ref().clone(),
                        status: status.as_ref().clone(),
                        headers,
                    }),
                    (None, _) => Err("body is required in http response mapping".to_string()),
                    (_, None) => Err("status is required in http response mapping".to_string()),
                }
            }
            _ => Err("response mapping must be a record".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expression::Expr;
    use crate::worker_binding::ResponseMapping;

    #[test]
    fn test_try_from_response_mapping() {
        let response_mapping = ResponseMapping(Expr::Record(
            vec![
                (
                    "body".to_string(),
                    Box::new(Expr::Variable("function.return".to_string())),
                ),
                (
                    "status".to_string(),
                    Box::new(Expr::Literal("200".to_string())),
                ),
                (
                    "headers".to_string(),
                    Box::new(Expr::Record(vec![(
                        "Content-Type".to_string(),
                        Box::new(Expr::Literal("application/json".to_string())),
                    )])),
                ),
            ]
            .into_iter()
            .collect(),
        ));

        let http_response_mapping = HttpResponseMapping::try_from(&response_mapping).unwrap();

        assert_eq!(
            http_response_mapping.body,
            Expr::Variable("function.return".to_string())
        );
        assert_eq!(
            http_response_mapping.status,
            Expr::Literal("200".to_string())
        );
        assert_eq!(http_response_mapping.headers.len(), 1);
        assert_eq!(
            http_response_mapping.headers.get("Content-Type").unwrap(),
            &Expr::Literal("application/json".to_string())
        );
    }

    #[test]
    fn test_try_from_response_mapping_missing_body() {
        let response_mapping = ResponseMapping(Expr::Record(
            vec![
                (
                    "status".to_string(),
                    Box::new(Expr::Literal("200".to_string())),
                ),
                (
                    "headers".to_string(),
                    Box::new(Expr::Record(vec![(
                        "Content-Type".to_string(),
                        Box::new(Expr::Literal("application/json".to_string())),
                    )])),
                ),
            ]
            .into_iter()
            .collect(),
        ));

        let result = HttpResponseMapping::try_from(&response_mapping);

        assert_eq!(
            result,
            Err("body is required in http response mapping".to_string())
        );
    }

    #[test]
    fn test_try_from_response_mapping_missing_status() {
        let response_mapping = ResponseMapping(Expr::Record(
            vec![
                (
                    "body".to_string(),
                    Box::new(Expr::Variable("function.return".to_string())),
                ),
                (
                    "headers".to_string(),
                    Box::new(Expr::Record(vec![(
                        "Content-Type".to_string(),
                        Box::new(Expr::Literal("application/json".to_string())),
                    )])),
                ),
            ]
            .into_iter()
            .collect(),
        ));

        let result = HttpResponseMapping::try_from(&response_mapping);

        assert_eq!(
            result,
            Err("status is required in http response mapping".to_string())
        );
    }

    #[test]
    fn test_try_from_response_mapping_headers_not_object() {
        let response_mapping = ResponseMapping(Expr::Record(
            vec![
                (
                    "body".to_string(),
                    Box::new(Expr::Variable("worker.response".to_string())),
                ),
                (
                    "status".to_string(),
                    Box::new(Expr::Literal("200".to_string())),
                ),
                (
                    "headers".to_string(),
                    Box::new(Expr::Literal("application/json".to_string())),
                ),
            ]
            .into_iter()
            .collect(),
        ));

        let result = HttpResponseMapping::try_from(&response_mapping);

        assert_eq!(result, Err("headers must be an object".to_string()));
    }

    #[test]
    fn test_try_from_response_mapping_not_record() {
        let response_mapping = ResponseMapping(Expr::Literal("200".to_string()));

        let result = HttpResponseMapping::try_from(&response_mapping);

        assert_eq!(result, Err("response mapping must be a record".to_string()));
    }
}

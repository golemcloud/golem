pub(crate) struct ExporterConfig {
    pub(crate) endpoint: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) service_name_mode: ServiceNameMode,
}

pub(crate) enum ServiceNameMode {
    AgentId,
    AgentType,
}

impl ExporterConfig {
    pub(crate) fn from_params(config: &[(String, String)]) -> Result<Option<Self>, String> {
        let endpoint = match config.iter().find(|(k, _)| k == "endpoint") {
            Some((_, v)) if v.is_empty() => {
                return Err("'endpoint' is configured but empty".to_string());
            }
            Some((_, v)) => v.clone(),
            None => return Ok(None),
        };

        if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
            return Err(format!(
                "'endpoint' must start with http:// or https://, got: '{endpoint}'"
            ));
        }

        let headers = match config.iter().find(|(k, _)| k == "headers") {
            Some((_, v)) => {
                let mut parsed = Vec::new();
                for pair in v.split(',') {
                    let mut parts = pair.splitn(2, '=');
                    let key = parts.next().unwrap().trim().to_string();
                    match parts.next() {
                        Some(val) => parsed.push((key, val.trim().to_string())),
                        None => {
                            return Err(format!(
                                "malformed header entry: '{}', expected 'key=value' format",
                                pair
                            ));
                        }
                    }
                }
                parsed
            }
            None => Vec::new(),
        };

        let service_name_mode = match config.iter().find(|(k, _)| k == "service-name-mode") {
            Some((_, v)) if v == "agent-id" => ServiceNameMode::AgentId,
            Some((_, v)) if v == "agent-type" => ServiceNameMode::AgentType,
            Some((_, v)) => {
                return Err(format!(
                    "invalid 'service-name-mode' value: '{}', expected 'agent-id' or 'agent-type'",
                    v
                ));
            }
            None => ServiceNameMode::AgentId,
        };

        Ok(Some(ExporterConfig {
            endpoint,
            headers,
            service_name_mode,
        }))
    }
}

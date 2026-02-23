use std::collections::HashMap;
use std::time::Duration;

#[derive(serde::Deserialize)]
pub struct HttpRequest {
    pub url: String,
    pub method: String,
    pub headers: HashMap<String, String>,
    pub body: Option<String>,
    pub timeout_secs: Option<u64>,
}

#[derive(serde::Serialize)]
pub struct HttpResponse {
    pub status: u16,
    pub status_text: String,
    pub body: String,
}

#[tauri::command]
pub async fn http_fetch(request: HttpRequest) -> Result<HttpResponse, String> {
    let timeout = Duration::from_secs(request.timeout_secs.unwrap_or(30));
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| e.to_string())?;

    let method = request
        .method
        .parse::<reqwest::Method>()
        .map_err(|e| e.to_string())?;

    let mut req = client.request(method, &request.url);

    for (key, value) in &request.headers {
        req = req.header(key, value);
    }

    if let Some(body) = request.body {
        req = req.body(body.trim().to_string());
    }

    let response = req.send().await.map_err(|e| e.to_string())?;

    let status = response.status();
    let result = HttpResponse {
        status: status.as_u16(),
        status_text: status.canonical_reason().unwrap_or("").to_string(),
        body: response.text().await.map_err(|e| e.to_string())?,
    };

    Ok(result)
}

use bytes::Bytes;
use reqwest::Client;
use serde::Deserialize;

#[derive(Clone)]
pub struct WorkerFilesClient {
    client: Client,
    base_url: String,
    token: Option<String>,
}

impl WorkerFilesClient {
    pub fn new(base_url: impl Into<String>, token: Option<String>) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token,
        }
    }

    pub async fn list_files(
        &self,
        component_id: &str,
        worker_name: &str,
        file_name: &str,
    ) -> anyhow::Result<GetFilesResponse> {
        let url = format!(
            "{}/{}/workers/{}/files/{}",
            self.base_url,
            percent_encoding::utf8_percent_encode(component_id, percent_encoding::NON_ALPHANUMERIC),
            percent_encoding::utf8_percent_encode(worker_name, percent_encoding::NON_ALPHANUMERIC),
            percent_encoding::utf8_percent_encode(file_name, percent_encoding::NON_ALPHANUMERIC)
        );
        let mut req = self.client.get(url);
        if let Some(t) = &self.token {
            req = req.bearer_auth(t);
        }
        let res = req.send().await?.error_for_status()?;
        let body = res.json::<GetFilesResponse>().await?;
        Ok(body)
    }

    pub async fn get_file_content(
        &self,
        component_id: &str,
        worker_name: &str,
        file_name: &str,
    ) -> anyhow::Result<Bytes> {
        let url = format!(
            "{}/{}/workers/{}/file-contents/{}",
            self.base_url,
            percent_encoding::utf8_percent_encode(component_id, percent_encoding::NON_ALPHANUMERIC),
            percent_encoding::utf8_percent_encode(worker_name, percent_encoding::NON_ALPHANUMERIC),
            percent_encoding::utf8_percent_encode(file_name, percent_encoding::NON_ALPHANUMERIC)
        );
        let mut req = self.client.get(url);
        if let Some(t) = &self.token {
            req = req.bearer_auth(t);
        }
        let res = req.send().await?.error_for_status()?;
        let bytes = res.bytes().await?;
        Ok(bytes)
    }
}

#[derive(Deserialize, Debug)]
pub struct GetFilesResponse {
    pub nodes: Vec<Node>,
}

#[derive(Deserialize, Debug)]
pub struct Node {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}
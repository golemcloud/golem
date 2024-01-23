use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum TemplateStoreConfig {
    S3(TemplateStoreS3Config),
    Local(TemplateStoreLocalConfig),
}

#[derive(Clone, Debug, Deserialize)]
pub struct TemplateStoreS3Config {
    pub bucket_name: String,
    pub object_prefix: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TemplateStoreLocalConfig {
    pub root_path: String,
    pub object_prefix: String,
}

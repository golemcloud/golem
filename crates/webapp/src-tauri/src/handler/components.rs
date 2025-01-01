use std::path::PathBuf;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use golem_cli::factory::ServiceFactory;
use golem_cli::model::component::Component;
use golem_cli::model::{ComponentName, Format, GolemError, GolemResult, PathBufOrStdin};
use golem_cli::model::app_ext_raw::ComponentType;
use crate::FACTORY;

#[tauri::command]
pub async fn get_component() -> GolemResult<Value, GolemError> {
    let service = FACTORY.component_service();
    Ok(service.list(None, None).await?.as_json_value())
    // let components =
    // components
}

// #[tauri::command]
// pub async fn get_component_by_id(id: String) -> Result<Value, GolemError> {
//     let service = FACTORY.component_service();
//     let components = service.get((name), None, None).await?.as_json_value();
//     Ok(components)
// }


// #[tauri::command]
// async fn create_component(name: String, path_buf: String, com_type: String, format: String) -> Result<Component, GolemError> {
//     let service = FACTORY.component_service();
//     let components = service.add(
//         ComponentName(name),
//         PathBufOrStdin::Path(PathBuf::from(path_buf)),
//         ComponentType::from(com_type),
//         None,
//         false,
//         Format::from(format),
//         vec![],
//     ).await;
//     components
// }

// pub fn component_handlers() -> Vec<Box<dyn Fn(tauri::AppHandle<tauri::Wry>, &str) -> Result<String, String> + Send + Sync>> {
//     tauri::generate_handler![
//         create_component,
//         get_component,
//     ]
// }



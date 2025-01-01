use crate::FACTORY;
use golem_cli::factory::ServiceFactory;
use golem_cli::model::{GolemError};
use serde::{Deserializer, Serialize};
use serde_json::Value;

#[derive(Serialize)]
pub enum Status {
    Success,
    Error,
}

#[derive(Serialize)]
pub struct Response<T> {
    status: Status,
    data: Option<T>,
    error: Option<String>,
}


// create the error type that represents all errors possible in our program
#[derive(Debug, thiserror::Error)]
enum GError {
    #[error(transparent)]
    Error(#[from] std::io::Error),
    #[error(transparent)]
    GolemError(#[from] GolemError)
}

// we must manually implement serde::Serialize
impl serde::Serialize for GError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::ser::Serializer,
    {
        serializer.serialize_str(self.to_string().as_ref())
    }
}

fn handle_response<T>(result: Result<T, GError>) -> Response<T> {
    match result {
        Ok(data) => {
            Response {
                status: Status::Success,
                data: Some(data),
                error: None,
            }
        },
        Err(error) => Response {
            status: Status::Error,
            data: None,
            error: Some(error.to_string()),
        },
    }
}


#[tauri::command]
pub async fn get_components() -> Response<Value> {
    let service = FACTORY.component_service();
    let result = service.list(None, None).await.map_err(|e| GError::GolemError(e));
    match result.map(|r| r.as_json_value()){
        Ok(data) => handle_response(Ok(data)),
        Err(error) => handle_response(Err(error))
    }
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



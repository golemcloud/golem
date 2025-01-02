use std::fs::File;
use std::io::Write;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use golem_cli::config::{get_config_dir, Config, NamedProfile, OssProfile, Profile, ProfileName};
use golem_cli::init::CliKind;
use golem_cli::oss::factory::OssServiceFactory;

#[derive(Serialize)]
pub enum Status {
    Success,
    Error,
}

#[derive(Serialize)]
pub struct Response {
    pub status: Status,
    pub data: Option<Value>,
    pub error: Option<String>,
}




lazy_static::lazy_static! {
    static ref PROFILE: (ProfileName, OssProfile) = get_profile();
    static ref FACTORY: OssServiceFactory = OssServiceFactory::from_profile(&PROFILE.1).unwrap();

}

fn get_profile() -> (ProfileName, OssProfile) {
    let config_dir = get_config_dir();
    let oss_profile = match Config::get_active_profile(CliKind::Oss, &config_dir) {
        Some(NamedProfile {
                 name,
                 profile: Profile::Golem(p),
             }) => Some((name, p)),
        Some(NamedProfile {
                 name: _,
                 profile: Profile::GolemCloud(_),
             }) => {
            /// TODO: Add a error log for the user
            None
        }
        None => None,
    }.unwrap();
    oss_profile
}

#[tauri::command]
async fn invoke_api(url: String, method: String, data: Option<Value>) -> Response {
    let endpoint = format!("{}{}", PROFILE.1.url.as_str(), url);
    println!("endpoint {:#?}", endpoint);
    println!("method {:#?}", method);
    println!("data {:#?}", data);

    let result = reqwest::Client::new()
        .request(match method.as_str() {
            "GET" => reqwest::Method::GET,
            "POST" => reqwest::Method::POST,
            "PUT" => reqwest::Method::PUT,
            "DELETE" => reqwest::Method::DELETE,
            _ => reqwest::Method::GET,
        }, endpoint)
        .header("Content-Type", "application/json")
        .send()
        .await;
    match result {
        Ok(data) => {
            // println!("result {:#?}", data.text().await);
            if data.status().is_success() {
                Response {
                    status: Status::Success,
                    data: Some(data.json().await.unwrap_or(Value::Null)),
                    error: None,
                }
            } else {
                Response {
                    status: Status::Error,
                    data: None,
                    error: Some(data.text().await.unwrap_or_default()),
                }
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
async fn process_multipart(name: String, file: Vec<u8>, filename: String) -> Result<String, String> {
    let file_path = format!("./uploads/{}", filename);
    let mut output = File::create(&file_path).map_err(|e| e.to_string())?;
    output.write(&file).map_err(|e| e.to_string())?;

    // Process additional form fields
    Ok(format!("Data received: {}, File saved at: {}", name, file_path))
}

// #[tauri::command]
// fn process_form(data: FormData) -> String {
//     let data = format!(
//         "Received form: Name: {}, Email: {}, Message: {}",
//         data.name, data.email, data.message
//     );
//     println!("{data}");
//     data
// }


// #[tauri::command]
// async fn invoke_form_data_api(url: String, method: String, data: FormData) -> Response {
//     let result = api::call_form_data_api(url, method, data).await;
//     match result {
//         Ok(data) => {
//             if data.status().is_success() {
//                 Response {
//                     status: Status::Success,
//                     data: Some(data.text().await.unwrap_or_default()),
//                     error: None,
//                 }
//             } else {
//                 Response {
//                     status: Status::Error,
//                     data: None,
//                     error: Some(data.text().await.unwrap_or_default()),
//                 }
//             }
//         },
//         Err(error) => Response {
//             status: Status::Error,
//             data: None,
//             error: Some(error.to_string()),
//         },
//     }
// }

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_http::init())
        .invoke_handler(tauri::generate_handler![invoke_api, process_multipart])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

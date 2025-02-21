use std::sync::Mutex;
use tauri::State;

struct Storage {
    backend_url: Mutex<String>,
}
#[tauri::command]
fn update_backend_ip(new_ip: String, store: State<Storage>) -> Result<(), String> {
    // Save the new IP as a JSON value under the key "backend_ip"
    // store.
    println!("Updated backend IP to: {}", new_ip);
    *store.backend_url.lock().unwrap() = new_ip;
    Ok(())
}

#[tauri::command]
fn get_backend_ip(store: State<Storage>) -> Result<String, String> {
    // Retrieve the stored IP, if any, and convert it back to a string.
    Ok(store.backend_url.lock().unwrap().to_string())
}

// #[tauri::command]
// fn update_ip(url: &str) -> Result<(), String> {
//     let config_path = Path::new("config.json");
//
//     // Load existing config or create a new one
//     let mut config = match fs::read_to_string(&config_path) {
//         Ok(content) => serde_json::from_str::<serde_json::Value>(&content).unwrap_or_default(),
//         Err(_) => serde_json::json!({}),
//     };
//
//     // Update the IP address
//     config["ip_address"] = serde_json::json!(url);
//
//     // Save back to file
//     fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap())
//         .map_err(|err| err.to_string())?;
//
//     Ok(())
// }
//
// fn load_ip() -> String {
//     let config_path = Path::new("config.json");
//     if let Ok(content) = fs::read_to_string(config_path) {
//         if let Ok(config) = serde_json::from_str::<serde_json::Value>(&content) {
//             if let Some(ip) = config["ip_address"].as_str() {
//                 return ip.to_string();
//             }
//         }
//     }
//     "http://localhost:9881".to_string() // Default IP
// }

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Create an instance of the store.
    tauri::Builder::default()
        .manage(Storage {
            backend_url: Mutex::from("http://localhost:9881".to_string()),
        })
        .plugin(tauri_plugin_store::Builder::default().build())
        .invoke_handler(tauri::generate_handler![update_backend_ip, get_backend_ip])
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_http::init())
        .plugin(tauri_plugin_websocket::init())
        .plugin(tauri_plugin_fs::init())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

use golem_cli::config::{get_config_dir, Config, NamedProfile, OssProfile, Profile, ProfileName};
use golem_cli::factory::ServiceFactory;
use golem_cli::init::CliKind;
use golem_cli::oss::factory::OssServiceFactory;
use crate::handler::components::get_component;
// use crate::handler::handlers;
// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/

mod handler;


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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![get_component])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

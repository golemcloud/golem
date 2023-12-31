use cloud_server_oss::api::make_open_api_service;
use cloud_server_oss::service::Services;

fn main() {
    let service = make_open_api_service(&Services::noop());
    println!("{}", service.spec_yaml())
}

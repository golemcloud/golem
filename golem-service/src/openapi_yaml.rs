use golem_service::api::make_open_api_service;
use golem_service::service::Services;

fn main() {
    let service = make_open_api_service(&Services::noop());
    println!("{}", service.spec_yaml())
}

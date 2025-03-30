mod bindings;

use bindings::golem::rpc::types::Uri;

use bindings::exports::rpc_grpc::api_caller_exports::rpc_grpc_api_caller_api::*;

use bindings::rpc_grpc::grpcbin_client::grpcbin_world_client::GRPCBinResource;
use bindings::rpc_grpc::grpcbin_exports::g_r_p_c_bin::GrpcConfiguration;
use bindings::rpc_grpc::grpcbin_exports::grpcbin::EmptyMessage;

struct Component;

impl Guest for Component {
    fn hello(_input: Input) -> Output {

        let empty_message = EmptyMessage { empty : true};
        let grpc_configuration = GrpcConfiguration {
            url: "http://grpcb.in:9000".to_string(), // we can get from env
            secret_token: "secret tokken".to_string()
        };

        let grpc_bin = GRPCBinResource::new(
            &Uri {value: "grpc".to_string()},
            &grpc_configuration
        ); // URI is not Used and to be remove

        let result_empty = grpc_bin.blocking_index(empty_message);
        match result_empty {
            Ok(index_reply) => {
                return Output{name: format!("Good, its fine and description received  : {}", index_reply.description.unwrap())};
            }
            Err(grpc_status) => {
                return Output{name: format!("Bad, but happy always, {}", grpc_status.message)};
            },
        }
    }
}

bindings::export!(Component with_types_in bindings);


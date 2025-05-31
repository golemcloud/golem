mod bindings;

use std::env;

use bindings::exports::rpc_grpc::api_caller_exports::rpc_grpc_api_caller_api::*;
use bindings::rpc_grpc::grpcbin_client::grpcbin_world_client::{
    DummyBiStreamResourceBidirectionalStreaming, DummyClientStreamResourceClientStreaming,
    DummyServerStreamResourceServerStreaming, GRPCBinResourceUnary
};
use bindings::rpc_grpc::grpcbin::grpcbin::{
    DummyMessage, DummyMessageEnum, DummyMessageSub, EmptyMessage,
};
use bindings::rpc_grpc::grpcbin_client::grpcbin_world_client::RpcGrpcConfiguration;

struct Component;

impl Guest for Component {
    fn unary_rpc() -> Output {
        let dummy_message = DummyMessage {
            f_string: Some("string".to_string()),
            f_strings: vec!["string".to_string()],
            f_int32: Some(1),
            f_int32s: vec![1],
            f_enums: vec![DummyMessageEnum::Enum1],
            f_enum: Some(DummyMessageEnum::Enum1),
            f_sub: Some(DummyMessageSub { f_string: None }),
            f_subs: vec![DummyMessageSub { f_string: None }],
            f_bool: Some(false),
            f_bools: vec![false],
            f_int64: Some(400000000),
            f_int64s: vec![40000000000],
            f_bytes: Some(vec![1, 1, 1, 2, 2, 4, 6, 7, 89]),
            f_bytess: vec![vec![1, 1, 1, 2, 2, 4, 6, 7, 8]],
            f_float: Some(15.6),
            f_floats: vec![14.2, 13.13],
        };

        let grpc_configuration = RpcGrpcConfiguration {
            url: env::var("RPC_GRPC_URL").expect("GRPC SERVER URL"),
            secret_token: env::var("RPC_GRPC_SECRET_TOKEN").ok(),
        };

        let service1 = GRPCBinResourceUnary::new(&grpc_configuration);

        match service1.blocking_dummy_unary(&dummy_message) {
            Ok(message) => {
                return Output {
                    name: format!("Good, received messaged : {:?}", message),
                };
            }
            Err(status) => {
                return Output {
                    name: format!("oh, send failed with status : {:?}", status),
                };
            }
        }
    }

    // Bi streaming rpc
    fn bidirectional_streaming_rpc() -> Output {
        let dummy_message = DummyMessage {
            f_string: Some("string".to_string()),
            f_strings: vec!["string".to_string()],
            f_int32: Some(1),
            f_int32s: vec![1],
            f_enums: vec![DummyMessageEnum::Enum1],
            f_enum: Some(DummyMessageEnum::Enum1),
            f_sub: Some(DummyMessageSub { f_string: None }),
            f_subs: vec![DummyMessageSub { f_string: None }],
            f_bool: Some(false),
            f_bools: vec![false],
            f_int64: Some(400000000),
            f_int64s: vec![40000000000],
            f_bytes: Some(vec![1, 1, 1, 2, 2, 4, 6, 7, 89]),
            f_bytess: vec![vec![1, 1, 1, 2, 2, 4, 6, 7, 8]],
            f_float: Some(15.6),
            f_floats: vec![14.2, 13.13],
        };
        let grpc_configuration = RpcGrpcConfiguration {
            url: env::var("RPC_GRPC_URL").expect("GRPC SERVER URL"),
            secret_token: env::var("RPC_GRPC_SECRET_TOKEN").ok(),
        };

        let service1 = DummyBiStreamResourceBidirectionalStreaming::new(&grpc_configuration);

        for _ in vec![1, 2, 3, 4] {
            match service1.blocking_send(&dummy_message) {
                Ok(is_success) => {
                    match is_success {
                        Some(_) => {
                            // success, message sent
                            // continue
                        }
                        None => {
                            // closed
                            break;
                        }
                    }
                }
                Err(_) => {
                    // error
                    return Output {
                        name: format!("oh, send failed"),
                    };
                }
            }

            match service1.blocking_receive() {
                Ok(message) => {
                    match message {
                        Some(_) => {
                            //
                        }
                        None => {
                            // closed, end
                            break;
                        }
                    }
                }
                Err(_) => {
                    return Output {
                        name: format!("oh, receive, failed"),
                    };
                }
            }
        }

        match service1.blocking_finish() {
            Ok(_) => {
                //
                return Output {
                    name: format!("Good, bi directional streaming was demostrated well enough"),
                };
            }
            Err(_) => {
                return Output {
                    name: format!("oh, finish failed"),
                };
            }
        }
    }

    // server streaming rpc
    fn server_streaming_rpc() -> Output {
        let dummy_message = DummyMessage {
            f_string: Some("string".to_string()),
            f_strings: vec!["string".to_string()],
            f_int32: Some(1),
            f_int32s: vec![1],
            f_enums: vec![DummyMessageEnum::Enum1],
            f_enum: Some(DummyMessageEnum::Enum1),
            f_sub: Some(DummyMessageSub { f_string: None }),
            f_subs: vec![DummyMessageSub { f_string: None }],
            f_bool: Some(false),
            f_bools: vec![false],
            f_int64: Some(400000000),
            f_int64s: vec![40000000000],
            f_bytes: Some(vec![1, 1, 1, 2, 2, 4, 6, 7, 89]),
            f_bytess: vec![vec![1, 1, 1, 2, 2, 4, 6, 7, 8]],
            f_float: Some(15.6),
            f_floats: vec![14.2, 13.13],
        };

        let grpc_configuration = RpcGrpcConfiguration {
            url: env::var("RPC_GRPC_URL").expect("GRPC SERVER URL"),
            secret_token: env::var("RPC_GRPC_SECRET_TOKEN").ok(),
        };

        let service1 = DummyServerStreamResourceServerStreaming::new(&grpc_configuration);

        let mut messages = vec![];

        match service1.blocking_send(&dummy_message) {
            Ok(is_success) => {
                match is_success {
                    Some(_) => {
                        let mut ok_continue = true;
                        while ok_continue {
                            match service1.blocking_receive() {
                                Ok(message) => match message {
                                    Some(message) => {
                                        messages.push(message);
                                    }
                                    None => {
                                        ok_continue = false;
                                    }
                                },
                                Err(_) => {
                                    return Output {
                                        name: format!("oh, receive, failed"),
                                    };
                                }
                            }
                        }
                    }
                    None => {
                        // will not reach here
                    }
                }
            }
            Err(grp_status) => {
                return Output {
                    name: format!("oh, send failed, {:?}", grp_status),
                };
            }
        };

        match service1.blocking_finish() {
            Ok(_) => {
                if messages.len() > 0 {
                    return Output {
                        name: format!(
                            "Good, received {} messages : {:?}",
                            messages.len(),
                            messages
                        ),
                    };
                } else {
                    return Output {
                        name: format!("oh, no messages are received"),
                    };
                }
            }
            Err(_) => {
                return Output {
                    name: format!("oh, finish failed"),
                };
            }
        }
    }

    fn client_streaming_rpc() -> Output {
        let dummy_message = DummyMessage {
            f_string: Some("string".to_string()),
            f_strings: vec!["string".to_string()],
            f_int32: Some(1),
            f_int32s: vec![1],
            f_enums: vec![DummyMessageEnum::Enum1],
            f_enum: Some(DummyMessageEnum::Enum1),
            f_sub: Some(DummyMessageSub { f_string: None }),
            f_subs: vec![DummyMessageSub { f_string: None }],
            f_bool: Some(false),
            f_bools: vec![false],
            f_int64: Some(400000000),
            f_int64s: vec![40000000000],
            f_bytes: Some(vec![1, 1, 1, 2, 2, 4, 6, 7, 89]),
            f_bytess: vec![vec![1, 1, 1, 2, 2, 4, 6, 7, 8]],
            f_float: Some(15.6),
            f_floats: vec![14.2, 13.13],
        };

        let grpc_configuration = RpcGrpcConfiguration {
            url: env::var("RPC_GRPC_URL").expect("GRPC SERVER URL"),
            secret_token: env::var("RPC_GRPC_SECRET_TOKEN").ok(),
        };

        let service1 = DummyClientStreamResourceClientStreaming::new(&grpc_configuration);

        for _ in vec![1, 2, 3, 4] {
            match service1.blocking_send(&dummy_message) {
                Ok(is_success) => {
                    match is_success {
                        Some(_) => {
                            // success, message sent
                            // continue
                        }
                        None => {
                            // closed
                            break;
                        }
                    }
                }
                Err(_grpc_status) => {
                    // error
                    return Output {
                        name: format!("oh, send failed"),
                    };
                }
            }
        }

        match service1.blocking_finish() {
            Ok(_) => {
                //
                return Output {
                    name: format!("Good, client streaming was demostrated well enough"),
                };
            }
            Err(_grpc_status) => {
                return Output {
                    name: format!("oh, finish failed"),
                };
            }
        }
    }

    fn empty_unary_rpc() -> Output {
        let empty_message = EmptyMessage { empty: true };

        let grpc_configuration = RpcGrpcConfiguration {
            url: env::var("RPC_GRPC_URL").expect("GRPC SERVER URL"),
            secret_token: env::var("RPC_GRPC_SECRET_TOKEN").ok(),
        };

        let service1 = GRPCBinResourceUnary::new(&grpc_configuration);

        match service1.blocking_empty_unary(empty_message) {
            Ok(message) => {
                return Output {
                    name: format!("Good, received messaged : {:?}", message),
                };
            }
            Err(status) => {
                return Output {
                    name: format!("oh, send failed with status : {:?}", status),
                };
            }
        }
    }
}

bindings::export!(Component with_types_in bindings);

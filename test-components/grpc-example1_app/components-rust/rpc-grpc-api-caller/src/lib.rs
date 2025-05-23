mod bindings;

use bindings::exports::rpc_grpc::api_caller_exports::rpc_grpc_api_caller_api::*;
use bindings::rpc_grpc::grpcbin::grpcbin::{EmptyMessage, GrpcConfiguration};
// use bindings::rpc_grpc::grpcbin_client::grpcbin_world_client::DummyServerStreamResourceServerStreaming;
// use bindings::rpc_grpc::grpcbin_client::grpcbin_world_client::DummyClientStreamResourceClientStreaming;
// use bindings::rpc_grpc::grpcbin_client::grpcbin_world_client::DummyBiStreamResourceBidirectionalStreaming;
use bindings::rpc_grpc::grpcbin_client::grpcbin_world_client::GRPCBinResourceUnary;

struct Component;

impl Guest for Component {
    fn hello() -> Output {
        
        let empty_message  = EmptyMessage {empty: true};

        let grpc_configuration = GrpcConfiguration {
            url: "http://localhost:50051".to_string(),
            secret_token: "secret tokken".to_string()
        };

        let service1 = GRPCBinResourceUnary::new(
            &grpc_configuration
        );

        match service1.blocking_dummy_unary(empty_message) {
            Ok(message) => {
                return Output{name: format!("Good, received messaged : {:?}", message)};
            }
            Err(status) => {
                return Output{name: format!("oh, send failed with status : {:?}", status)};
            }
        }
    }

    
    // // Bi streaming rpc
    // fn hello() -> Output {

    //     let empty_message  = EmptyMessage {empty: true};

    //     let grpc_configuration = GrpcConfiguration {
    //         url: "http://localhost:50051".to_string(),
    //         secret_token: "secret tokken".to_string()
    //     };

    //     let service1 = DummyBiStreamResourceBidirectionalStreaming::new(
    //         &grpc_configuration
    //     );

    //     for _ in vec![1,2,3,4] {
    //         match service1.blocking_send(empty_message) {
    //             Ok(is_success) => {
    //                 match is_success {
    //                     Some(_) => {
    //                         // success, message sent 
    //                         // continue
    //                     }
    //                     None => {
    //                         // closed
    //                         break;
    //                     }
    //                 }
    //             },
    //             Err(grpc_status) => {
    //                 // error
    //                 println!("send failed, grpc_status : {:?}", grpc_status);
    //                 return Output{name: format!("oh, send failed")};
    //             },
    //         }

    //         match service1.blocking_receive() {
    //             Ok(message) => {
    //                 match message {
    //                     Some(_) => {
    //                         //
    //                     }
    //                     None => {
    //                         // closed, end
    //                         break;
    //                     }
    //                 }
    //             },
    //             Err(grpc_status) => {
                    
    //                 println!("receive failed, grpc_status : {:?}", grpc_status);
    //                 return Output{name: format!("oh, receive, failed")};
    //             },
    //         }
    //     }

    //     match service1.blocking_finish() {
    //         Ok(_) => {
    //             //
    //             return Output{name: format!("Good, bi directional streaming was demostrated well enough")};
    //         }
    //         Err(grpc_status) => {
    //             println!("finish failed, grpc_status : {:?}", grpc_status);
    //             return Output{name: format!("oh, finish failed")};
    //         },
    //     }
    // }

// // server streaming rpc
// fn hello() -> Output {

//     let empty_message  = EmptyMessage {empty: true};

//     let grpc_configuration = GrpcConfiguration {
//         url: "http://localhost:50051".to_string(),
//         secret_token: "secret tokken".to_string()
//     };

//     let service1 = DummyServerStreamResourceServerStreaming::new(
//         &grpc_configuration
//     );

//     let mut messages = vec![];

//     match service1.blocking_send(empty_message) {
//         Ok(is_success) => {
//             match is_success {
//                 Some(_) => {
//                     let mut ok_continue = true;
//                     while ok_continue {
//                         match service1.blocking_receive() {
//                             Ok(message) => {
//                                 match message {
//                                     Some(message) => {
//                                         messages.push(message);
//                                     }
//                                     None => {
//                                         ok_continue = false;
//                                     }
//                                 }
//                             },
//                             Err(grpc_status) => {
                                
//                                 println!("receive failed, grpc_status : {:?}", grpc_status);
//                                 return Output{name: format!("oh, receive, failed")};
//                             },
//                         }
//                     }
//                 }
//                 None => {
//                     // will not reach here
//                 }
//             }
//         },
//         Err(grpc_status) => {
//             println!("send failed, grpc_status : {:?}", grpc_status);
//             return Output{name: format!("oh, send failed")};
//         },
//     };

//     match service1.blocking_finish() {
//         Ok(_) => {
//             if messages.len()>0 {
//                 return Output{name: format!("Good, received {} messages : {:?}", messages.len(), messages)};
//             } else {
//                 return Output{name: format!("oh, no messages are received")};
//             }
//         }
//         Err(grpc_status) => {
//             println!("finish failed, grpc_status : {:?}", grpc_status);
//             return Output{name: format!("oh, finish failed")};
//         },
//     }
// }

}

bindings::export!(Component with_types_in bindings);

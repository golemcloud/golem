
mod bindings;
use bindings::exports::rpc_grpc::grpcbin_exports::g_r_p_c_bin::{Guest, GuestGRPCBinResource}; 

struct Component;

impl Guest for Component {
    type GRPCBinResource  = Component;
}

impl GuestGRPCBinResource for Component {
    fn new(grpc_configuration: bindings::exports::rpc_grpc::grpcbin_exports::g_r_p_c_bin::GrpcConfiguration) -> Self {
        todo!()
    }

    fn index(
        &self,
        empty_message: bindings::exports::rpc_grpc::grpcbin_exports::g_r_p_c_bin::EmptyMessage,
    ) -> Result<bindings::exports::rpc_grpc::grpcbin_exports::g_r_p_c_bin::IndexReply, bindings::exports::rpc_grpc::grpcbin_exports::g_r_p_c_bin::GrpcStatus> {
        todo!()
    }

    fn empty(
        &self,
        empty_message: bindings::exports::rpc_grpc::grpcbin_exports::g_r_p_c_bin::EmptyMessage,
    ) -> Result<bindings::exports::rpc_grpc::grpcbin_exports::g_r_p_c_bin::EmptyMessage, bindings::exports::rpc_grpc::grpcbin_exports::g_r_p_c_bin::GrpcStatus> {
        todo!()
    }

    fn dummy_unary(
        &self,
        dummy_message: bindings::exports::rpc_grpc::grpcbin_exports::g_r_p_c_bin::DummyMessage,
    ) -> Result<bindings::exports::rpc_grpc::grpcbin_exports::g_r_p_c_bin::DummyMessage, bindings::exports::rpc_grpc::grpcbin_exports::g_r_p_c_bin::GrpcStatus> {
        todo!()
    }

    fn dummy_server_stream(
        &self,
        dummy_message: bindings::exports::rpc_grpc::grpcbin_exports::g_r_p_c_bin::DummyMessage,
    ) -> Result<bindings::exports::rpc_grpc::grpcbin_exports::g_r_p_c_bin::DummyMessage, bindings::exports::rpc_grpc::grpcbin_exports::g_r_p_c_bin::GrpcStatus> {
        todo!()
    }

    fn dummy_client_stream(
        &self,
        dummy_message: bindings::exports::rpc_grpc::grpcbin_exports::g_r_p_c_bin::DummyMessage,
    ) -> Result<bindings::exports::rpc_grpc::grpcbin_exports::g_r_p_c_bin::DummyMessage, bindings::exports::rpc_grpc::grpcbin_exports::g_r_p_c_bin::GrpcStatus> {
        todo!()
    }

    fn dummy_bidirectional_stream_stream(
        &self,
        dummy_message: bindings::exports::rpc_grpc::grpcbin_exports::g_r_p_c_bin::DummyMessage,
    ) -> Result<bindings::exports::rpc_grpc::grpcbin_exports::g_r_p_c_bin::DummyMessage, bindings::exports::rpc_grpc::grpcbin_exports::g_r_p_c_bin::GrpcStatus> {
        todo!()
    }

    fn specific_error(
        &self,
        specific_error_request: bindings::exports::rpc_grpc::grpcbin_exports::g_r_p_c_bin::SpecificErrorRequest,
    ) -> Result<bindings::exports::rpc_grpc::grpcbin_exports::g_r_p_c_bin::EmptyMessage, bindings::exports::rpc_grpc::grpcbin_exports::g_r_p_c_bin::GrpcStatus> {
        todo!()
    }

    fn random_error(
        &self,
        empty_message: bindings::exports::rpc_grpc::grpcbin_exports::g_r_p_c_bin::EmptyMessage,
    ) -> Result<bindings::exports::rpc_grpc::grpcbin_exports::g_r_p_c_bin::EmptyMessage, bindings::exports::rpc_grpc::grpcbin_exports::g_r_p_c_bin::GrpcStatus> {
        todo!()
    }

    fn headers_unary(
        &self,
        empty_message: bindings::exports::rpc_grpc::grpcbin_exports::g_r_p_c_bin::EmptyMessage,
    ) -> Result<bindings::exports::rpc_grpc::grpcbin_exports::g_r_p_c_bin::HeadersMessage, bindings::exports::rpc_grpc::grpcbin_exports::g_r_p_c_bin::GrpcStatus> {
        todo!()
    }

    fn no_response_unary(
        &self,
        empty_message: bindings::exports::rpc_grpc::grpcbin_exports::g_r_p_c_bin::EmptyMessage,
    ) -> Result<bindings::exports::rpc_grpc::grpcbin_exports::g_r_p_c_bin::EmptyMessage, bindings::exports::rpc_grpc::grpcbin_exports::g_r_p_c_bin::GrpcStatus> {
        todo!()
    }
}

bindings::export!(Component with_types_in bindings);

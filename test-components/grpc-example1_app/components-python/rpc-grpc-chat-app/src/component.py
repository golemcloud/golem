from wit_world import exports
from wit_world.imports.grpcbin import DummyMessage,DummyMessageSub, DummyMessageEnum
from wit_world.imports.grpcbin_world_client import GRPCBinResourceUnary
from wit_world.exports.rpc_grpc_chat_app_api import *;
from wit_world.imports.grpcbin_world_client import rpc_grpc;
# Example common lib import
# from lib import example_common_function

class RpcGrpcChatAppApi(exports.RpcGrpcChatAppApi):
    def chat1(self, name: str) -> str:
        grpc_configuration = rpc_grpc.GrpcConfiguration()
        grpc_configuration.url= "http://localhost:50051"
        grpc_configuration.secret_token = "token"

        dummy_message = DummyMessage()
        dummy_message.f_bool = False
        dummy_message.f_bools = [False, False]
        dummy_message.f_bytes = "rama".encode("utf-8")
        dummy_message.f_bytess = ["rama".encode("utf-8"),"rama".encode("utf-8")]
        dummy_message.f_enum = DummyMessageEnum.ENUM1
        dummy_message.f_enums = [DummyMessageEnum.ENUM1, DummyMessageEnum.ENUM1]
        dummy_message.f_float = 99.999
        dummy_message.f_floats = [99.9999, 3.14]
        dummy_message.f_int32 = 16
        dummy_message.f_int32s = [16, 17,18,19,20]
        dummy_message.f_int64 = 10000000
        dummy_message.f_int64s = [100000000, 123456789]
        dummy_message.f_string = "string1"
        dummy_message.f_strings = ["string1", "string1"]
        dummy_message.f_sub = DummyMessageSub(f_string="string1")
        dummy_message.f_subs = [DummyMessageSub(f_string="string1")]

        unary_server = GRPCBinResourceUnary(grpc_configuration);
        result = unary_server.blocking_dummy_unary(dummy_message)

        return f"Hi, {name}, dummy_message is {result}";

    def chat2(self, name: str) -> str:
        return f"Hi, {name}";

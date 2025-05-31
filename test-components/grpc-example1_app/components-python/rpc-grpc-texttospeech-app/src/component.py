from wit_world import exports
from wit_world import imports;
from wit_world.exports import *;
from wit_world.imports.google_cloud_texttospeech_v1_world_client import TextToSpeechResourceUnary;
# from wit_world.imports import google_cloud_texttospeech_v1;
from wit_world.imports import google_cloud_texttospeech_v1_world_client;
from wit_world.imports.text_to_speech import google_cloud_texttospeech_v1;
from wit_world.imports import google_protobuf;
# Example common lib import
# from lib import example_common_function



class RpcGrpcTexttospeechAppApi(exports.RpcGrpcTexttospeechAppApi):
    def get() -> str :
        grpc_configuration = google_cloud_texttospeech_v1_world_client.rpc_grpc.GrpcConfiguration()
        grpc_configuration.url= "http://localhost:50051"
        grpc_configuration.secret_token = "token";

        rpc = TextToSpeechResourceUnary(grpc_configuration)
        list_voices_request = google_cloud_texttospeech_v1.ListVoicesRequest();
        list_voices_request.language_code = None
        resp= rpc.blocking_list_voices(list_voices_request)
        
        return resp.voices[0].name

class LazyDescriptorProto(google_protobuf.LazyDescriptorProto):
    def _init_() -> None:
        return None;


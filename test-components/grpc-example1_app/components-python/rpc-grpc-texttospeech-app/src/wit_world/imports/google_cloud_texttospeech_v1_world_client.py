from typing import TypeVar, Generic, Union, Optional, Protocol, Tuple, List, Any, Self
from types import TracebackType
from enum import Flag, Enum, auto
from dataclasses import dataclass
from abc import abstractmethod
import weakref

from ..types import Result, Ok, Err, Some
from ..imports import google_cloud_texttospeech_v1
from ..imports import google_protobuf
from ..imports import rpc_grpc
from ..imports import google_api

class GoogleCloudTexttospeechV1:
    
    def __init__(self) -> None:
        raise NotImplementedError

    def __enter__(self) -> Self:
        """Returns self"""
        return self
                                
    def __exit__(self, exc_type: type[BaseException] | None, exc_value: BaseException | None, traceback: TracebackType | None) -> bool | None:
        """
        Release this resource.
        """
        raise NotImplementedError


class RpcGrpc:
    
    def __init__(self) -> None:
        raise NotImplementedError

    def __enter__(self) -> Self:
        """Returns self"""
        return self
                                
    def __exit__(self, exc_type: type[BaseException] | None, exc_value: BaseException | None, traceback: TracebackType | None) -> bool | None:
        """
        Release this resource.
        """
        raise NotImplementedError


class TextToSpeech:
    
    def __init__(self) -> None:
        raise NotImplementedError

    def __enter__(self) -> Self:
        """Returns self"""
        return self
                                
    def __exit__(self, exc_type: type[BaseException] | None, exc_value: BaseException | None, traceback: TracebackType | None) -> bool | None:
        """
        Release this resource.
        """
        raise NotImplementedError


class TextToSpeechResourceUnary:
    
    def __init__(self, grpc_configuration: rpc_grpc.GrpcConfiguration) -> None:
        raise NotImplementedError

    def blocking_list_voices(self, google_cloud_texttospeech_v1_list_voices_request: google_cloud_texttospeech_v1.ListVoicesRequest) -> google_cloud_texttospeech_v1.ListVoicesResponse:
        """
        Raises: `wit_world.types.Err(wit_world.imports.rpc_grpc.GrpcStatus)`
        """
        raise NotImplementedError
    def blocking_synthesize_speech(self, google_cloud_texttospeech_v1_synthesize_speech_request: google_cloud_texttospeech_v1.SynthesizeSpeechRequest) -> google_cloud_texttospeech_v1.SynthesizeSpeechResponse:
        """
        Raises: `wit_world.types.Err(wit_world.imports.rpc_grpc.GrpcStatus)`
        """
        raise NotImplementedError
    def __enter__(self) -> Self:
        """Returns self"""
        return self
                                
    def __exit__(self, exc_type: type[BaseException] | None, exc_value: BaseException | None, traceback: TracebackType | None) -> bool | None:
        """
        Release this resource.
        """
        raise NotImplementedError


class StreamingSynthesizeResourceBidirectionalStreaming:
    
    def __init__(self, grpc_configuration: rpc_grpc.GrpcConfiguration) -> None:
        raise NotImplementedError

    def blocking_send(self, message: google_cloud_texttospeech_v1.StreamingSynthesizeRequest) -> Optional[bool]:
        """
        Raises: `wit_world.types.Err(wit_world.imports.rpc_grpc.GrpcStatus)`
        """
        raise NotImplementedError
    def blocking_receive(self) -> Optional[google_cloud_texttospeech_v1.StreamingSynthesizeResponse]:
        """
        Raises: `wit_world.types.Err(wit_world.imports.rpc_grpc.GrpcStatus)`
        """
        raise NotImplementedError
    def blocking_finish(self) -> bool:
        """
        Raises: `wit_world.types.Err(wit_world.imports.rpc_grpc.GrpcStatus)`
        """
        raise NotImplementedError
    def __enter__(self) -> Self:
        """Returns self"""
        return self
                                
    def __exit__(self, exc_type: type[BaseException] | None, exc_value: BaseException | None, traceback: TracebackType | None) -> bool | None:
        """
        Release this resource.
        """
        raise NotImplementedError


class GoogleProtobuf:
    
    def __init__(self) -> None:
        raise NotImplementedError

    def __enter__(self) -> Self:
        """Returns self"""
        return self
                                
    def __exit__(self, exc_type: type[BaseException] | None, exc_value: BaseException | None, traceback: TracebackType | None) -> bool | None:
        """
        Release this resource.
        """
        raise NotImplementedError


class LazyDescriptorProto:
    
    def __init__(self, descriptor_proto: google_protobuf.DescriptorProto) -> None:
        raise NotImplementedError

    def blocking_get(self) -> google_protobuf.DescriptorProto:
        raise NotImplementedError
    def __enter__(self) -> Self:
        """Returns self"""
        return self
                                
    def __exit__(self, exc_type: type[BaseException] | None, exc_value: BaseException | None, traceback: TracebackType | None) -> bool | None:
        """
        Release this resource.
        """
        raise NotImplementedError


class GoogleApi:
    
    def __init__(self) -> None:
        raise NotImplementedError

    def __enter__(self) -> Self:
        """Returns self"""
        return self
                                
    def __exit__(self, exc_type: type[BaseException] | None, exc_value: BaseException | None, traceback: TracebackType | None) -> bool | None:
        """
        Release this resource.
        """
        raise NotImplementedError


class LazyHttpRule:
    
    def __init__(self, http_rule: google_api.HttpRule) -> None:
        raise NotImplementedError

    def blocking_get(self) -> google_api.HttpRule:
        raise NotImplementedError
    def __enter__(self) -> Self:
        """Returns self"""
        return self
                                
    def __exit__(self, exc_type: type[BaseException] | None, exc_value: BaseException | None, traceback: TracebackType | None) -> bool | None:
        """
        Release this resource.
        """
        raise NotImplementedError




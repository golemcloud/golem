from typing import TypeVar, Generic, Union, Optional, Protocol, Tuple, List, Any, Self
from types import TracebackType
from enum import Flag, Enum, auto
from dataclasses import dataclass
from abc import abstractmethod
import weakref

from ..types import Result, Ok, Err, Some
from ..imports import rpc_grpc
from ..imports import grpcbin

class GRPCBinResourceUnary:
    
    def __init__(self, grpc_configuration: rpc_grpc.GrpcConfiguration) -> None:
        raise NotImplementedError

    def dummy_unary(self, grpcbin_dummy_message: grpcbin.DummyMessage) -> grpcbin.DummyMessage:
        """
        Raises: `wit_world.types.Err(wit_world.imports.rpc_grpc.GrpcStatus)`
        """
        raise NotImplementedError
    def empty_unary(self, grpcbin_empty_message: grpcbin.EmptyMessage) -> grpcbin.DummyMessage:
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


class DummyServerStreamResourceServerStreaming:
    
    def __init__(self, grpc_configuration: rpc_grpc.GrpcConfiguration) -> None:
        raise NotImplementedError

    def send(self, message: grpcbin.DummyMessage) -> Optional[bool]:
        """
        Raises: `wit_world.types.Err(wit_world.imports.rpc_grpc.GrpcStatus)`
        """
        raise NotImplementedError
    def receive(self) -> Optional[grpcbin.DummyMessage]:
        """
        Raises: `wit_world.types.Err(wit_world.imports.rpc_grpc.GrpcStatus)`
        """
        raise NotImplementedError
    def finish(self) -> bool:
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


class DummyClientStreamResourceClientStreaming:
    
    def __init__(self, grpc_configuration: rpc_grpc.GrpcConfiguration) -> None:
        raise NotImplementedError

    def send(self, message: grpcbin.DummyMessage) -> Optional[bool]:
        """
        Raises: `wit_world.types.Err(wit_world.imports.rpc_grpc.GrpcStatus)`
        """
        raise NotImplementedError
    def finish(self) -> grpcbin.DummyMessage:
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


class DummyBiStreamResourceBidirectionalStreaming:
    
    def __init__(self, grpc_configuration: rpc_grpc.GrpcConfiguration) -> None:
        raise NotImplementedError

    def send(self, message: grpcbin.DummyMessage) -> Optional[bool]:
        """
        Raises: `wit_world.types.Err(wit_world.imports.rpc_grpc.GrpcStatus)`
        """
        raise NotImplementedError
    def receive(self) -> Optional[grpcbin.DummyMessage]:
        """
        Raises: `wit_world.types.Err(wit_world.imports.rpc_grpc.GrpcStatus)`
        """
        raise NotImplementedError
    def finish(self) -> bool:
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




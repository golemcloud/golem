from typing import TypeVar, Generic, Union, Optional, Protocol, Tuple, List, Any, Self
from types import TracebackType
from enum import Flag, Enum, auto
from dataclasses import dataclass
from abc import abstractmethod
import weakref

from ..types import Result, Ok, Err, Some


class RpcGrpcChatAppApi(Protocol):

    @abstractmethod
    def chat1(self, message: str) -> str:
        raise NotImplementedError

    @abstractmethod
    def chat2(self, message: str) -> str:
        raise NotImplementedError



from typing import TypeVar, Generic, Union, Optional, Protocol, Tuple, List, Any, Self
from types import TracebackType
from enum import Flag, Enum, auto
from dataclasses import dataclass
from abc import abstractmethod
import weakref

from ..types import Result, Ok, Err, Some


@dataclass
class HeadersMessageValues:
    values: List[str]

@dataclass
class HeadersMessageMetadataEntry:
    key: Optional[str]
    value: Optional[HeadersMessageValues]

@dataclass
class HeadersMessage:
    metadata: List[HeadersMessageMetadataEntry]

@dataclass
class SpecificErrorRequest:
    code: Optional[int]
    reason: Optional[str]

@dataclass
class EmptyMessage:
    empty: bool

@dataclass
class DummyMessageSub:
    f_string: Optional[str]

class DummyMessageEnum(Enum):
    ENUM1 = 0

@dataclass
class DummyMessage:
    f_string: Optional[str]
    f_strings: List[str]
    f_int32: Optional[int]
    f_int32s: List[int]
    f_enum: Optional[DummyMessageEnum]
    f_enums: List[DummyMessageEnum]
    f_sub: Optional[DummyMessageSub]
    f_subs: List[DummyMessageSub]
    f_bool: Optional[bool]
    f_bools: List[bool]
    f_int64: Optional[int]
    f_int64s: List[int]
    f_bytes: Optional[bytes]
    f_bytess: List[bytes]
    f_float: Optional[float]
    f_floats: List[float]

@dataclass
class IndexReplyEndpoint:
    path: Optional[str]
    description: Optional[str]

@dataclass
class IndexReply:
    description: Optional[str]
    endpoints: List[IndexReplyEndpoint]



from typing import TypeVar, Generic, Union, Optional, Protocol, Tuple, List, Any, Self
from types import TracebackType
from enum import Flag, Enum, auto
from dataclasses import dataclass
from abc import abstractmethod
import weakref


S = TypeVar('S')
@dataclass
class Some(Generic[S]):
    value: S

T = TypeVar('T')
@dataclass
class Ok(Generic[T]):
    value: T

E = TypeVar('E')
@dataclass(frozen=True)
class Err(Generic[E], Exception):
    value: E

Result = Union[Ok[T], Err[E]]

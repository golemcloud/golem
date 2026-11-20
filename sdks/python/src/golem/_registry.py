"""Agent declarations.

`@agent` and `@method` only record the decorated class or function and where
it is defined. The bootstrap finalizes the registry once, after it imports
every entry module and before pre-initialization saves the component.
Finalization resolves the annotations in the scopes that define them and
produces one declaration per agent type, which discovery and the argument
and result checks share. Neither decoration nor finalization constructs an
agent.

Finalization reports every invalid declaration together, each with the
location of its definition.
"""

import inspect
import sys
import typing
from collections.abc import Callable
from dataclasses import dataclass, field
from types import FunctionType
from typing import Any

import annotationlib

INT_MIN = -(2**63)
INT_MAX = 2**63 - 1


@dataclass(frozen=True)
class Kind:
    """A supported value type and its schema type."""

    name: str
    python_name: str

    def check(self, value: Any) -> Any:
        """Returns `value` when it has this type; raises `TypeError` otherwise."""
        if self is BOOL:
            ok = isinstance(value, bool)
        elif self is S64:
            ok = isinstance(value, int) and not isinstance(value, bool)
            if ok and not INT_MIN <= value <= INT_MAX:
                raise TypeError(
                    f"{value} is out of the range of int, which is a signed 64-bit integer"
                )
        elif self is F64:
            ok = isinstance(value, float)
        elif self is STRING:
            ok = isinstance(value, str)
        else:
            ok = isinstance(value, bytes)
        if not ok:
            raise TypeError(f"expected {self.python_name}, got {type(value).__name__}")
        return value


BOOL = Kind("bool", "bool")
S64 = Kind("s64", "int")
F64 = Kind("f64", "float")
STRING = Kind("string", "str")
BINARY = Kind("binary", "bytes")

_KINDS = ((bool, BOOL), (int, S64), (float, F64), (str, STRING), (bytes, BINARY))


def _kind(hint: Any) -> Kind | None:
    """The kind of a supported hint. Compares identities, because a hint such
    as `Annotated[int, {}]` is unhashable."""
    return next((kind for python, kind in _KINDS if hint is python), None)


_SUPPORTED = "bool, int, float, str and bytes"
_SUPPORTED_RESULTS = "bool, int, float, str, bytes and None"


@dataclass(frozen=True)
class Parameter:
    name: str
    kind: Kind


@dataclass(frozen=True)
class MethodDeclaration:
    name: str
    function: Callable[..., Any]
    parameters: tuple[Parameter, ...]
    # `None` when the method returns `None`.
    output: Kind | None

    def check_result(self, value: Any) -> Any:
        if self.output is None:
            if value is not None:
                raise TypeError(f"expected None, got {type(value).__name__}")
            return None
        return self.output.check(value)


@dataclass(frozen=True)
class AgentDeclaration:
    name: str
    cls: type
    parameters: tuple[Parameter, ...]
    methods: dict[str, MethodDeclaration]


class DeclarationError(Exception):
    """One or more agent declarations are invalid."""

    def __init__(self, errors: list[str]):
        self.errors = errors
        count = (
            "1 invalid agent declaration"
            if len(errors) == 1
            else (f"{len(errors)} invalid agent declarations")
        )
        super().__init__(count + ":\n" + "\n".join(f"- {error}" for error in errors))


@dataclass
class _Agent:
    target: Any
    location: str


@dataclass
class Registry:
    agents: list[_Agent] = field(default_factory=list)
    methods: list[tuple[Any, str]] = field(default_factory=list)
    errors: list[str] = field(default_factory=list)
    declarations: tuple[AgentDeclaration, ...] | None = None

    def finalize(self) -> tuple[AgentDeclaration, ...]:
        if self.declarations is not None:
            raise RuntimeError("the agent registry is already finalized")
        errors = list(self.errors)
        declarations = []
        claimed: set[int] = set()
        names: dict[str, str] = {}
        for entry in self.agents:
            declaration = _declare_agent(entry, errors, claimed)
            if declaration is None:
                continue
            if declaration.name in names:
                errors.append(
                    f"{entry.location}: agent type `{declaration.name}` is already "
                    f"declared at {names[declaration.name]}"
                )
                continue
            names[declaration.name] = entry.location
            declarations.append(declaration)
        for function, location in self.methods:
            if id(function) not in claimed:
                errors.append(
                    f"{location}: `{function.__qualname__}` is decorated with @method, "
                    "but its class is not decorated with @agent"
                )
        if errors:
            raise DeclarationError(errors)
        self.declarations = tuple(declarations)
        return self.declarations

    def finalized(self) -> tuple[AgentDeclaration, ...]:
        if self.declarations is None:
            raise RuntimeError("the agent registry is not finalized")
        return self.declarations


current = Registry()


def _caller_location() -> str:
    frame = sys._getframe(2)
    return f"{frame.f_code.co_filename}:{frame.f_lineno}"


def _function_location(function: Any) -> str:
    code = getattr(function, "__code__", None)
    if code is None:
        return "<unknown>"
    return f"{code.co_filename}:{code.co_firstlineno}"


def agent(cls: Any = None, /):
    """Declares an agent type. Its name is the class name.

    The constructor is the class's ordinary `__init__`; its parameters are the
    agent's constructor parameters. Methods decorated with `@method` are the
    agent's methods.
    """
    if cls is None:
        location = _caller_location()

        def decorate(cls: Any) -> Any:
            current.agents.append(_Agent(cls, location))
            return cls

        return decorate
    current.agents.append(_Agent(cls, _caller_location()))
    return cls


def method(function: Any = None, /):
    """Declares an agent method. Its name is the function name."""
    if function is None:
        location = _caller_location()

        def decorate(function: Any) -> Any:
            _record_method(function, location)
            return function

        return decorate
    _record_method(function, _caller_location())
    return function


def _record_method(function: Any, location: str) -> None:
    if not isinstance(function, FunctionType):
        current.errors.append(
            f"{location}: @method must decorate a function defined in the class body, "
            f"not {type(function).__name__}"
        )
        return
    function.__golem_method__ = True
    current.methods.append((function, _function_location(function)))


def _declare_agent(
    entry: _Agent, errors: list[str], claimed: set[int]
) -> AgentDeclaration | None:
    cls = entry.target
    if not isinstance(cls, type):
        errors.append(
            f"{entry.location}: @agent must decorate a class, not {type(cls).__name__}"
        )
        return None
    before = len(errors)
    name = cls.__name__

    try:
        class_annotations = annotationlib.get_annotations(
            cls, format=annotationlib.Format.FORWARDREF
        )
    except Exception as error:
        errors.append(
            f"{entry.location}: cannot evaluate the class annotations of agent "
            f"`{name}`: {type(error).__name__}: {error}"
        )
        class_annotations = {}
    if "config" in class_annotations:
        errors.append(
            f"{entry.location}: agent `{name}` declares `config`, but agent config is "
            "not supported yet"
        )

    init = cls.__dict__.get("__init__")
    if init is None:
        constructor: tuple[Parameter, ...] = ()
        if any("__init__" in vars(base) for base in cls.__mro__[1:-1]):
            errors.append(
                f"{entry.location}: agent `{name}` inherits its constructor; "
                "define `__init__` in the agent class"
            )
    elif getattr(init, "__golem_method__", False):
        errors.append(
            f"{_function_location(init)}: `{name}.__init__` is the constructor and "
            "cannot be decorated with @method"
        )
        claimed.add(id(init))
        constructor = ()
    else:
        constructor = _parameters(f"the constructor of `{name}`", init, errors, False)[
            0
        ]

    methods = {}
    for attribute, value in cls.__dict__.items():
        target = value
        if isinstance(value, (staticmethod, classmethod)):
            target = value.__func__
        if not getattr(target, "__golem_method__", False) or attribute == "__init__":
            continue
        claimed.add(id(target))
        what = f"method `{name}.{attribute}`"
        if target is not value:
            errors.append(
                f"{_function_location(target)}: {what} must be an instance method, "
                f"not a {type(value).__name__}"
            )
            continue
        parameters, output = _parameters(what, target, errors, True)
        methods[attribute] = MethodDeclaration(attribute, target, parameters, output)

    if len(errors) > before:
        return None
    return AgentDeclaration(name, cls, constructor, methods)


def _parameters(
    what: str, function: Any, errors: list[str], has_output: bool
) -> tuple[tuple[Parameter, ...], Kind | None]:
    location = _function_location(function)
    if inspect.iscoroutinefunction(function) or inspect.isasyncgenfunction(function):
        errors.append(
            f"{location}: {what} is async, but async methods are not supported yet"
        )
        return (), None
    if inspect.isgeneratorfunction(function):
        errors.append(f"{location}: {what} is a generator, which is not supported")
        return (), None
    try:
        # Keep `Annotated` and similar wrappers, which are not supported yet,
        # instead of reducing them to the type that they wrap.
        hints = typing.get_type_hints(function, include_extras=True)
    except Exception as error:
        errors.append(
            f"{location}: cannot resolve the annotations of {what}: "
            f"{type(error).__name__}: {error}"
        )
        return (), None

    signature = inspect.signature(function)
    receiver, *rest = list(signature.parameters.values()) or [None]
    if receiver is None or receiver.kind not in (
        receiver.POSITIONAL_ONLY,
        receiver.POSITIONAL_OR_KEYWORD,
    ):
        errors.append(
            f"{location}: {what} must take the agent instance as its first "
            "positional parameter"
        )
        return (), None
    parameters = []
    for parameter in rest:
        label = f"parameter `{parameter.name}` of {what}"
        if parameter.kind in (parameter.VAR_POSITIONAL, parameter.VAR_KEYWORD):
            errors.append(f"{location}: {label} is variadic, which is not supported")
            continue
        if parameter.kind is parameter.KEYWORD_ONLY:
            errors.append(
                f"{location}: {label} is keyword-only, which is not supported"
            )
            continue
        if parameter.default is not parameter.empty:
            errors.append(
                f"{location}: {label} has a default value, which is not supported yet"
            )
            continue
        if parameter.name not in hints:
            errors.append(f"{location}: {label} has no type annotation")
            continue
        kind = _kind(hints[parameter.name])
        if kind is None:
            errors.append(
                f"{location}: {label} has the unsupported type "
                f"`{_type_name(hints[parameter.name])}`; supported types are {_SUPPORTED}"
            )
            continue
        parameters.append(Parameter(parameter.name, kind))
    output = None
    if has_output:
        if "return" not in hints:
            errors.append(f"{location}: {what} has no return type annotation")
        elif hints["return"] is not type(None):
            output = _kind(hints["return"])
            if output is None:
                errors.append(
                    f"{location}: {what} returns the unsupported type "
                    f"`{_type_name(hints['return'])}`; supported types are "
                    f"{_SUPPORTED_RESULTS}"
                )
    return tuple(parameters), output


def _type_name(hint: Any) -> str:
    if isinstance(hint, type):
        return hint.__qualname__
    return repr(hint)


def finalize() -> None:
    current.finalize()


def declarations() -> tuple[AgentDeclaration, ...]:
    return current.finalized()

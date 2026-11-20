"""Agent initialization and method dispatch.

A component instance hosts at most one agent. `initialize` constructs it once
with the ordinary constructor; later calls fail without changing it. `invoke`
calls a declared method on that agent, which keeps its ordinary object state
between calls.

The caller decodes the arguments against the declared parameters before any
user code runs. A result of the wrong type fails the invocation after the
method has run; the method's effects remain.
"""

from collections.abc import Callable, Sequence
from typing import Any

from golem._registry import AgentDeclaration, Kind, MethodDeclaration, Parameter

Decode = Callable[[Sequence[Parameter]], list[Any]]


class InvalidType(Exception):
    pass


class InvalidMethod(Exception):
    pass


class InvalidInput(Exception):
    pass


class Failure(Exception):
    """A failure that the agent reports as a custom error message."""


def describe(error: BaseException) -> str:
    message = str(error)
    return f"{type(error).__name__}: {message}" if message else type(error).__name__


class Dispatcher:
    def __init__(self, declarations: Sequence[AgentDeclaration]):
        self._types = {declaration.name: declaration for declaration in declarations}
        self._declaration: AgentDeclaration | None = None
        self._instance: Any = None

    @property
    def declaration(self) -> AgentDeclaration | None:
        return self._declaration

    def initialize(self, type_name: str, decode: Decode) -> None:
        if self._declaration is not None:
            raise Failure(
                f"the agent is already initialized as `{self._declaration.name}`"
            )
        declaration = self._types.get(type_name)
        if declaration is None:
            known = ", ".join(f"`{name}`" for name in self._types) or "none"
            raise InvalidType(
                f"this component defines no agent type `{type_name}`; it defines {known}"
            )
        arguments = decode(declaration.parameters)
        try:
            instance = declaration.cls(*arguments)
        except Exception as error:
            raise Failure(
                f"the constructor of `{type_name}` failed: {describe(error)}"
            ) from error
        self._declaration = declaration
        self._instance = instance

    def invoke(self, method_name: str, decode: Decode) -> tuple[Kind | None, Any]:
        """Returns the method's output kind and its checked result."""
        if self._declaration is None:
            raise Failure(
                f"cannot invoke `{method_name}`: the agent is not initialized"
            )
        method: MethodDeclaration | None = self._declaration.methods.get(method_name)
        if method is None:
            raise InvalidMethod(
                f"agent type `{self._declaration.name}` has no method `{method_name}`"
            )
        arguments = decode(method.parameters)
        try:
            result = method.function(self._instance, *arguments)
        except Exception as error:
            raise Failure(f"`{method_name}` failed: {describe(error)}") from error
        try:
            return method.output, method.check_result(result)
        except TypeError as error:
            raise Failure(
                f"`{method_name}` returned an invalid result: {error}"
            ) from error

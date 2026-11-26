"""Implementations of the component's exports.

Importing this module registers them; the bootstrap imports it before the
application's entry modules.
"""

from collections.abc import Sequence
from typing import Any

from componentize_py_types import Err
from wit.exports.golem.agent_v2 import Guest as AgentGuest
from wit.exports.golem.agent_v2.guest import guest as agent_guest
from wit.exports.golem.tool_v0_1 import Guest as ToolGuest
from wit.exports.golem.tool_v0_1 import ToolMiddlewareGuest
from wit.exports.golem.tool_v0_1.guest import guest as tool_guest
from wit.exports.golem.tool_v0_1.tool_middleware_guest import (
    guest as tool_middleware_guest,
)
from wit.imports.golem.agent_v2 import common
from wit.imports.golem.core_v2 import types
from wit.imports.golem.core_v2.types import ToolError_InvalidToolName

from golem import _registry
from golem._dispatch import (
    Dispatcher,
    Failure,
    InvalidInput,
    InvalidMethod,
    InvalidType,
)
from golem._registry import (
    BINARY,
    BOOL,
    F64,
    S64,
    STRING,
    AgentDeclaration,
    Kind,
    Parameter,
)

_METADATA = types.MetadataEnvelope(
    doc=None, aliases=[], examples=[], deprecated=None, role=None
)

_TYPE_BODIES = {
    BOOL: lambda: types.SchemaTypeBody_BoolType(),
    S64: lambda: types.SchemaTypeBody_S64Type(None),
    F64: lambda: types.SchemaTypeBody_F64Type(None),
    STRING: lambda: types.SchemaTypeBody_StringType(),
    BINARY: lambda: types.SchemaTypeBody_BinaryType(
        types.BinaryRestrictions(mime_types=None, min_bytes=None, max_bytes=None)
    ),
}

_VALUE_NODES = {
    BOOL: (types.SchemaValueNode_BoolValue, "bool-value"),
    S64: (types.SchemaValueNode_S64Value, "s64-value"),
    F64: (types.SchemaValueNode_F64Value, "f64-value"),
    STRING: (types.SchemaValueNode_StringValue, "string-value"),
    BINARY: (types.SchemaValueNode_BinaryValue, "binary-value"),
}


class _Graph:
    def __init__(self) -> None:
        self.nodes: list[types.SchemaTypeNode] = []
        self._kinds: dict[Kind, int] = {}

    def push(self, body: Any) -> int:
        self.nodes.append(types.SchemaTypeNode(body=body, metadata=_METADATA))
        return len(self.nodes) - 1

    def kind(self, kind: Kind) -> int:
        if kind not in self._kinds:
            self._kinds[kind] = self.push(_TYPE_BODIES[kind]())
        return self._kinds[kind]

    def input(self, parameters: Sequence[Parameter]) -> common.InputSchema:
        return common.InputSchema_Parameters(
            [
                common.NamedField(
                    name=parameter.name,
                    source=common.FieldSource_UserSupplied(),
                    schema=self.kind(parameter.kind),
                    metadata=_METADATA,
                )
                for parameter in parameters
            ]
        )


def _agent_type(declaration: AgentDeclaration) -> common.AgentType:
    graph = _Graph()
    constructor = common.AgentConstructor(
        name=None,
        description="",
        prompt_hint=None,
        input_schema=graph.input(declaration.parameters),
    )
    methods = [
        common.AgentMethod(
            name=method.name,
            description="",
            http_endpoint=[],
            prompt_hint=None,
            input_schema=graph.input(method.parameters),
            output_schema=(
                common.OutputSchema_Unit()
                if method.output is None
                else common.OutputSchema_Single(graph.kind(method.output))
            ),
            read_only=None,
        )
        for method in declaration.methods.values()
    ]
    # The graph root is a required placeholder; the constructor and the
    # methods refer to their own nodes.
    root = graph.push(types.SchemaTypeBody_RecordType([]))
    return common.AgentType(
        type_name=declaration.name,
        kind=common.AgentTypeKind.REGULAR,
        description="",
        source_language="python",
        schema=types.SchemaGraph(type_nodes=graph.nodes, defs=[], root=root),
        constructor=constructor,
        methods=methods,
        dependencies=[],
        mode=common.AgentMode.DURABLE,
        http_mount=None,
        snapshotting=common.Snapshotting_Disabled(),
        config=[],
    )


def _decoder(tree: types.SchemaValueTree):
    def decode(parameters: Sequence[Parameter]) -> list[Any]:
        nodes = tree.value_nodes

        def node(index: int, what: str) -> Any:
            if not 0 <= index < len(nodes):
                raise InvalidInput(f"{what} refers to the missing value node {index}")
            return nodes[index]

        root = node(tree.root, "the input root")
        if not isinstance(root, types.SchemaValueNode_RecordValue):
            raise InvalidInput(
                f"the input root must be a record value, not {_node_name(root)}"
            )
        if len(root.value) != len(parameters):
            raise InvalidInput(
                f"expected {len(parameters)} parameters, got {len(root.value)}"
            )
        arguments = []
        for parameter, index in zip(parameters, root.value):
            value = node(index, f"parameter `{parameter.name}`")
            expected, name = _VALUE_NODES[parameter.kind]
            if not isinstance(value, expected):
                raise InvalidInput(
                    f"parameter `{parameter.name}` expects a {name}, "
                    f"got {_node_name(value)}"
                )
            arguments.append(
                value.value.bytes if parameter.kind is BINARY else value.value
            )
        return arguments

    return decode


def _node_name(node: Any) -> str:
    name = type(node).__name__.removeprefix("SchemaValueNode_")
    return "".join(
        "-" + c.lower() if c.isupper() and i else c.lower() for i, c in enumerate(name)
    )


def _encode(kind: Kind, value: Any) -> types.SchemaValueTree:
    node, _ = _VALUE_NODES[kind]
    if kind is BINARY:
        value = types.BinaryValuePayload(bytes=value, mime_type=None)
    return types.SchemaValueTree(value_nodes=[node(value)], root=0)


def _custom_error(message: str) -> common.AgentError_CustomError:
    return common.AgentError_CustomError(
        types.TypedSchemaValue(
            graph=types.SchemaGraph(
                type_nodes=[
                    types.SchemaTypeNode(
                        body=types.SchemaTypeBody_StringType(), metadata=_METADATA
                    )
                ],
                defs=[],
                root=0,
            ),
            value=types.SchemaValueTree(
                value_nodes=[types.SchemaValueNode_StringValue(message)], root=0
            ),
        )
    )


def _agent_error(error: Exception) -> Err:
    if isinstance(error, InvalidType):
        return Err(common.AgentError_InvalidType(str(error)))
    if isinstance(error, InvalidMethod):
        return Err(common.AgentError_InvalidMethod(str(error)))
    if isinstance(error, InvalidInput):
        return Err(common.AgentError_InvalidInput(str(error)))
    return Err(_custom_error(str(error)))


_dispatcher: Dispatcher | None = None


def _get_dispatcher() -> Dispatcher:
    global _dispatcher
    if _dispatcher is None:
        _dispatcher = Dispatcher(_registry.declarations())
    return _dispatcher


@agent_guest
class Agent(AgentGuest):
    async def initialize(self, agent_type, input):
        try:
            _get_dispatcher().initialize(agent_type, _decoder(input))
        except (InvalidType, InvalidInput, Failure) as error:
            raise _agent_error(error)

    async def invoke(self, method_name, input):
        try:
            kind, result = _get_dispatcher().invoke(method_name, _decoder(input))
        except (InvalidMethod, InvalidInput, Failure) as error:
            raise _agent_error(error)
        return None if kind is None else _encode(kind, result)

    def get_definition(self):
        declaration = _get_dispatcher().declaration
        if declaration is None:
            raise RuntimeError("the agent is not initialized")
        return _agent_type(declaration)

    def discover_agent_types(self):
        return [_agent_type(declaration) for declaration in _registry.declarations()]


@tool_guest
class Tools(ToolGuest):
    def discover_tools(self):
        return []

    def get_tool(self, name):
        raise Err(ToolError_InvalidToolName(name))

    async def invoke(self, tool_name, command_path, input, stdin, stdout, stderr):
        raise Err(ToolError_InvalidToolName(tool_name))


@tool_middleware_guest
class ToolMiddleware(ToolMiddlewareGuest):
    def discover_tool_middlewares(self):
        return []

    def get_tool_middleware(self, name):
        raise Err(ToolError_InvalidToolName(name))

    async def invoke_tool_middleware(self, middleware_name, *args):
        raise Err(ToolError_InvalidToolName(middleware_name))

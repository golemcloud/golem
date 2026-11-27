"""Implementations of the component's exports.

Importing this module registers them; the bootstrap imports it before the
application's entry modules.
"""

from componentize_py_types import Err
from wit.exports.golem.agent_v2 import Guest as AgentGuest
from wit.exports.golem.agent_v2.guest import guest as agent_guest
from wit.exports.golem.tool_v0_1 import Guest as ToolGuest, ToolMiddlewareGuest
from wit.exports.golem.tool_v0_1.guest import guest as tool_guest
from wit.exports.golem.tool_v0_1.tool_middleware_guest import (
    guest as tool_middleware_guest,
)
from wit.imports.golem.agent_v2.common import AgentError_InvalidType
from wit.imports.golem.core_v2.types import ToolError_InvalidToolName

from golem import _registry


def _unknown_agent_type(name: str) -> Err:
    return Err(AgentError_InvalidType(f"this component defines no agent type `{name}`"))


@agent_guest
class Agent(AgentGuest):
    async def initialize(self, agent_type, input):
        raise _unknown_agent_type(agent_type)

    async def invoke(self, method_name, input):
        raise Err(AgentError_InvalidType("the agent is not initialized"))

    def get_definition(self):
        raise RuntimeError("the agent is not initialized")

    def discover_agent_types(self):
        return _registry.agent_types()


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

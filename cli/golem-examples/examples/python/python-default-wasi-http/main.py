import asyncio
import poll_loop

from component_name import exports
from component_name.types import Ok
from component_name.imports import types
from component_name.imports.types import (
    Method_Get,
    Method_Post,
    Scheme,
    Scheme_Http,
    Scheme_Https,
    Scheme_Other,
    IncomingRequest,
    ResponseOutparam,
    OutgoingResponse,
    Fields,
    OutgoingBody,
    OutgoingRequest,
)
from poll_loop import Stream, Sink, PollLoop
from typing import Tuple
from urllib import parse

# see https://github.com/bytecodealliance/componentize-py/tree/main/examples/http for a full example.

class IncomingHandler(exports.IncomingHandler):
    """Implements the wasi:http/incoming-handler"""

    def handle(self, request: IncomingRequest, response_out: ResponseOutparam) -> None:
        """Handle the specified `request`, sending the response to `response_out`."""
        # Dispatch the request using `asyncio`, backed by a custom event loop
        # based on WASI's `poll_oneoff` function.
        loop = PollLoop()
        asyncio.set_event_loop(loop)
        loop.run_until_complete(handle_async(request, response_out))


async def handle_async(
    request: IncomingRequest, response_out: ResponseOutparam
) -> None:
    """Handle the specified `request`, sending the response to `response_out`."""

    method = request.method()
    path = request.path_with_query()
    headers = request.headers().entries()

    if isinstance(method, Method_Get) and path == "/hello":
        response = OutgoingResponse(Fields.from_list([]))

        response_body = response.body()

        ResponseOutparam.set(response_out, Ok(response))

        sink = Sink(response_body)
        await sink.send(bytes(f"Hello from python", "utf-8"))
        sink.close()
    else:
        response = OutgoingResponse(Fields.from_list([]))
        response.set_status_code(400)
        body = response.body()
        ResponseOutparam.set(response_out, Ok(response))
        OutgoingBody.finish(body, None)

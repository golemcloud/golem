import {
    ResponseOutparam,
    OutgoingBody,
    OutgoingResponse,
    Fields,
  } from 'wasi:http/types@0.2.0';

// For a detailed guide see https://github.com/bytecodealliance/jco/tree/main/examples/components/http-hello-world

/**
 * This export represents the `wasi:http/incoming-handler` interface,
 * which describes implementing a HTTP handler in WebAssembly using WASI types.
 */
export const incomingHandler = {
    /**
     * This Javascript will be turned into a WebAssembly component by `jco` and turned into a
     * WebAssembly binary with a single export (this `handler` function).
     *
     * The exported `handle` method is part of the `wasi:http/incoming-handler` interface,
     * which defines how to hadle incoming web requests, turning this component into one that can
     * serve web requests.
     */
    handle: function (incomingRequest, responseOutparam) {
        // Start building an outgoing response
        const outgoingResponse = new OutgoingResponse(new Fields());

        // Access the outgoing response body
        let outgoingBody = outgoingResponse.body();
        {
            // Create a stream for the response body
            let outputStream = outgoingBody.write();
            // Write hello world to the response stream
            outputStream.blockingWriteAndFlush(
                new Uint8Array(new TextEncoder().encode('Hello from Javascript!\n'))
            );
            // @ts-ignore: This is required in order to dispose the stream before we return
            outputStream[Symbol.dispose]();
        }

        // Set the status code for the response
        outgoingResponse.setStatusCode(200);
        // Finish the response body
        OutgoingBody.finish(outgoingBody, undefined);
        // Set the created response to an "OK" Result<T> value
        ResponseOutparam.set(outgoingResponse, { tag: 'ok', val: outgoingResponse });
    },
}

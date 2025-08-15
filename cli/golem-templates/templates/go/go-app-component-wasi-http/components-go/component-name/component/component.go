package main

import (
	incominghandler "app/components-go/component-name/binding/wasi/http/incoming-handler"

	"github.com/golemcloud/golem-go/binding/wasi/http/types"
	"go.bytecodealliance.org/cm"
)

func init() {
	incominghandler.Exports.Handle = Handle
}

// Implementation of the exported interface

func Handle(request incominghandler.IncomingRequest, responseOut incominghandler.ResponseOutparam) {
	// Construct HttpResponse to send back
	headers := types.NewFields()
	httpResponse := types.NewOutgoingResponse(headers)
	httpResponse.SetStatusCode(200)

	body, _, isErr := httpResponse.Body().Result()
	if isErr {
		panic("Failed to open response body")
	}
	stream, _, isErr := body.Write().Result()
	if isErr {
		panic("Failed to open response body stream")
	}
	_, err, isErr := stream.BlockingWriteAndFlush(cm.ToList([]uint8("Hello from Go!\n"))).Result()
	if isErr {
		panic("Failed to flush response body: " + err.String())
	}

	result := cm.OK[cm.Result[types.ErrorCodeShape, types.OutgoingResponse, types.ErrorCode]](httpResponse)
	types.ResponseOutparamSet(types.ResponseOutparam(responseOut), result)
}

func main() {}

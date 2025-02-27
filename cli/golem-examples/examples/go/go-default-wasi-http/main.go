package main

import (
	"pack/name/binding"
)

// Helper type aliases to make code more readable
type HttpRequest = binding.ExportsWasiHttp0_2_0_IncomingHandlerIncomingRequest
type HttpResponseOut = binding.ExportsWasiHttp0_2_0_IncomingHandlerResponseOutparam
type HttpOutgoingResponse = binding.WasiHttp0_2_0_TypesOutgoingResponse
type HttpError = binding.WasiHttp0_2_0_TypesErrorCode

func init() {
	binding.SetExportsWasiHttp0_2_0_IncomingHandler(&ComponentNameImpl{})
}

type ComponentNameImpl struct {}

// Implementation of the exported interface

func (e *ComponentNameImpl) Handle(request HttpRequest, responseOut HttpResponseOut) {
	// Construct HttpResponse to send back
	headers := binding.NewFields()
	httpResponse := binding.NewOutgoingResponse(headers)
	httpResponse.SetStatusCode(200)
	httpResponse.Body().Unwrap().Write().Unwrap().BlockingWriteAndFlush([]uint8("Hello from Go!\n")).Unwrap()

	// Send HTTP response
	okResponse := binding.Ok[HttpOutgoingResponse, HttpError](httpResponse)
	binding.StaticResponseOutparamSet(responseOut, okResponse)
}

func main() {}

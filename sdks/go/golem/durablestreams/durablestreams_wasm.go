// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//go:build wasip1

package durablestreams

import (
	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/internal/secretref"
	wire "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_durable_streams"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// withAuth lends the secret's handle to create while it runs; the host keeps
// the secret's identity, not the handle.
func withAuth[R any](auth golem.Option[golem.Secret[string]], create func(witTypes.Option[*types.Secret]) R) R {
	secret, ok := auth.Get()
	if !ok {
		return create(witTypes.None[*types.Secret]())
	}
	h, release, err := secretref.Borrow(secret)
	if err != nil {
		panic(err)
	}
	defer release()
	return create(witTypes.Some(h))
}

func hostReader(url string, mode wire.DurableStreamMode, options ReadOptions) readCall {
	reader := withAuth(options.Auth, func(auth witTypes.Option[*types.Secret]) *wire.DurableStreamReader {
		return wire.MakeDurableStreamReader(wire.DurableStreamReaderOptions{
			Url: url, Mode: mode, TimeoutMs: uint64(options.Timeout.Milliseconds()),
		}, auth)
	})
	return func(request wire.DurableStreamReadRequest) (wire.DurableStreamBatch, *Error) {
		res := reader.Read(request)
		if res.IsErr() {
			return wire.DurableStreamBatch{}, errorFromWire(res.Err())
		}
		return res.Ok(), nil
	}
}

func hostWriter(url, contentType, producerID string, options WriteOptions) appendCall {
	writer := withAuth(options.Auth, func(auth witTypes.Option[*types.Secret]) *wire.DurableStreamWriter {
		return wire.MakeDurableStreamWriter(wire.DurableStreamWriterOptions{
			Url: url, ContentType: contentType, ProducerId: producerID, ProducerEpoch: options.Epoch,
			TimeoutMs: uint64(options.Timeout.Milliseconds()),
		}, auth)
	})
	return func(request wire.DurableStreamAppendRequest) (wire.DurableStreamAppendReceipt, *Error) {
		res := writer.Append(request)
		if res.IsErr() {
			return wire.DurableStreamAppendReceipt{}, errorFromWire(res.Err())
		}
		return res.Ok(), nil
	}
}

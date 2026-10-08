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

package golem

import (
	"time"

	quota "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_quota_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// NewQuotaToken acquires a token for resourceName, declaring how much of it the
// agent expects to use; the runtime shares the resource among tokens in
// proportion to their expected use.
func NewQuotaToken(resourceName string, expectedUse uint64) QuotaToken {
	return QuotaToken{st: &quotaTokenState{h: quota.NewToken(resourceName, expectedUse)}}
}

// Reserve reserves amount before using it. When the resource throttles, it
// waits until the amount is available; when the resource rejects instead, the
// error is a [*FailedReservation].
func (t QuotaToken) Reserve(amount uint64) (*Reservation, error) {
	res := quota.Reserve(t.handle(), amount)
	if res.Tag() == witTypes.ResultErr {
		failed := &FailedReservation{EstimatedWait: None[time.Duration]()}
		if wait := res.Err().EstimatedWaitNanos; wait.IsSome() {
			failed.EstimatedWait = Some(time.Duration(wait.Some()))
		}
		return nil, failed
	}
	r := res.Ok()
	return &Reservation{commit: func(used uint64) { quota.ReservationCommit(r, used) }}, nil
}

// Split carves a child token off this one, with part of its expected use, for
// handing to another agent.
func (t QuotaToken) Split(childExpectedUse uint64) QuotaToken {
	return QuotaToken{st: &quotaTokenState{h: quota.Split(t.handle(), childExpectedUse)}}
}

// Merge folds other, a token for the same resource, back into this one; other
// is unusable afterwards.
func (t QuotaToken) Merge(other QuotaToken) {
	h := other.handle()
	other.st.h = nil
	quota.Merge(t.handle(), h)
}

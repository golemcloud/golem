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

func hostNewQuotaToken(resourceName string, expectedUse uint64) QuotaToken {
	return QuotaToken{st: &quotaTokenState{h: quota.NewToken(resourceName, expectedUse)}}
}

func (t QuotaToken) hostReserve(amount uint64) (*Reservation, error) {
	res := quota.Reserve(t.handle(), amount)
	if res.Tag() == witTypes.ResultErr {
		failed := &FailedReservation{EstimatedWait: None[time.Duration]()}
		if wait := res.Err().EstimatedWaitNanos; wait.IsSome() {
			failed.EstimatedWait = Some(durationFromNanos(wait.Some()))
		}
		return nil, failed
	}
	r := res.Ok()
	return &Reservation{commit: func(used uint64) { quota.ReservationCommit(r, used) }}, nil
}

func (t QuotaToken) hostSplit(childExpectedUse uint64) QuotaToken {
	return QuotaToken{st: &quotaTokenState{h: quota.Split(t.handle(), childExpectedUse)}}
}

func (t QuotaToken) hostMerge(other QuotaToken) {
	h := other.handle()
	other.st.h = nil
	quota.Merge(t.handle(), h)
}

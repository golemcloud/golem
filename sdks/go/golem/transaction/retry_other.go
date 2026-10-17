//go:build !wasip1

package transaction

import "time"

var sagaNow = func() uint64 { return uint64(time.Now().UnixNano()) }

var sagaNext = func([]byte, uint64, int64, string) ([]byte, time.Duration, bool, *PolicyError) {
	return nil, 0, false, &PolicyError{Message: "saga retries require the Golem host"}
}

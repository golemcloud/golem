package observer

import (
	"errors"
	"sync"
	"testing"
)

func TestDetachDisposesUnclaimedCapabilitiesBeforeDecode(t *testing.T) {
	for _, customError := range []bool{false, true} {
		entered, settle := make(chan struct{}), make(chan struct{})
		resources := []*int{new(int), new(int), new(int), new(int)}
		type raw struct {
			capabilities []*int
			customError  bool
		}
		drops, cancels, decodes := 0, 0, 0
		o := Observer[raw, int]{
			GetRaw: func() raw { close(entered); <-settle; return raw{resources, customError} },
			Decode: func(raw) (int, error) { decodes++; return 99, nil },
			Dispose: func(r raw) {
				if r.customError != customError {
					t.Error("wrong raw outcome")
				}
				for _, c := range r.capabilities {
					*c++
				}
			},
			Drop: func() { drops++ }, CancelRaw: func() { cancels++ },
		}
		done := make(chan struct{})
		go func() {
			defer close(done)
			got, err := o.Get()
			if got != 0 || !errors.Is(err, ErrDetached) {
				t.Errorf("late result %d, %v", got, err)
			}
		}()
		<-entered
		o.Detach()
		if drops != 0 || cancels != 0 {
			t.Fatal("detach dropped active Get or cancelled invocation")
		}
		close(settle)
		<-done
		if drops != 1 || decodes != 0 {
			t.Fatal("late outcome decoded or not released")
		}
		for _, c := range resources {
			if *c != 1 {
				t.Fatal("late capability not released exactly once")
			}
		}
		o.Cancel()
		o.Detach()
		if cancels != 0 || drops != 1 {
			t.Fatal("cancel reached physically detached observer")
		}
	}
}

func TestDropPanicWakesAllWaitersAndPreservesOriginalPanic(t *testing.T) {
	entered, settle := make(chan struct{}), make(chan struct{})
	o := Observer[int, int]{GetRaw: func() int { close(entered); <-settle; panic("original") }, Drop: func() { panic("drop") }}
	var wg sync.WaitGroup
	for range 4 {
		wg.Add(1)
		go func() {
			defer wg.Done()
			defer func() {
				if recover() != "original" {
					t.Error("original panic replaced")
				}
			}()
			_, _ = o.Get()
		}()
	}
	<-entered
	close(settle)
	wg.Wait()
}

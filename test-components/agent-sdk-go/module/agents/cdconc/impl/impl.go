// Package impl is the IMPLEMENTATION of the concurrent custom-durability agent.
// Every outbound call goes to the test's HTTP server on PORT, whose gates let a
// test order the goroutines causally rather than by timing.
package impl

import (
	crand "crypto/rand"
	"fmt"
	"io"
	"math/rand/v2"
	"net/http"
	"os"
	"runtime"
	"strings"
	"sync"
	"time"

	"agent-sdk-go/agents/cdconc"

	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/durability"
)

type state struct{}

var agent = cdconc.Agent.Implement(func(cdconc.Id) *state { return &state{} })

func get(path string) string {
	resp := golem.Must(http.Get("http://localhost:" + os.Getenv("PORT") + path))
	defer resp.Body.Close()
	return string(golem.Must(io.ReadAll(resp.Body)))
}

// allocateAroundBody runs a sequential body that allocates, then allocates and
// makes host calls on the same goroutine.
func allocateAroundBody() string {
	b := op("op", func() string {
		var sum int
		for i := range 3 {
			buf := make([]byte, 1<<20)
			buf[i] = byte(i + 1)
			sum += int(buf[i])
		}
		return get(fmt.Sprintf("/echo?v=b%d", sum))
	})
	var out strings.Builder
	for i := range 8 {
		buf := make([]byte, 1<<20)
		buf[i] = byte(i)
		out.WriteString(get(fmt.Sprintf("/echo?v=r%d", buf[i])))
	}
	return b + "," + out.String()
}

// spinPastRootTimer starts a root goroutine that sleeps on two Go timers in a
// row, then lets a body read the clock, without blocking on the host, until both
// are long due.
func spinPastRootTimer() string {
	done := make(chan string, 1)
	go func() {
		// The second timer is armed while run is already reading the clock, so
		// no host wait for it exists when it becomes due.
		time.Sleep(5 * time.Millisecond)
		time.Sleep(20 * time.Millisecond)
		done <- get("/echo?v=root")
	}()
	runtime.Gosched()
	b := op("op", func() string {
		start := time.Now()
		rounds, x := 0, uint64(1)
		for time.Since(start) < 80*time.Millisecond && rounds < 2000 {
			for range 2_000_000 {
				x = x*6364136223846793005 + 1442695040888963407
			}
			rounds++
			runtime.Gosched()
		}
		if x == 0 {
			return "zero"
		}
		return "b"
	})
	return b + "," + <-done
}

var remembered string

// drawAfterBody lets a body use maps, then iterates a map on the same goroutine,
// making one host call per key, and draws from math/rand. It keeps the result
// in memory, where a later invocation can read it back.
func drawAfterBody() string {
	b := op("op", func() string {
		seen := 0
		for i := range 17 {
			m := map[int]int{i: i, i + 1: i}
			for range m {
				seen++
			}
		}
		return fmt.Sprintf("b%d", seen)
	})
	keys := map[string]bool{"k1": true, "k2": true, "k3": true, "k4": true, "k5": true, "k6": true}
	var out strings.Builder
	for k := range keys {
		out.WriteString(k + "=" + get("/echo?v="+k) + ";")
	}
	remembered = fmt.Sprintf("%s,%s%d", b, out.String(), rand.Uint64())
	return remembered
}

func op(name string, body func() string) string {
	return durability.Run(
		durability.Spec{Interface: "cdconc", Function: name, Type: durability.WriteRemote},
		golem.Unit{},
		body,
	)
}

func init() {
	agent.Handle(cdconc.Siblings, func(_ *golem.Context[state], _ golem.Unit) string {
		var wg sync.WaitGroup
		res := make([]string, 2)
		wg.Add(2)
		go func() {
			defer wg.Done()
			res[0] = op("op", func() string { return get("/gate?name=sib") })
		}()
		go func() {
			defer wg.Done()
			res[1] = op("op", func() string {
				r := get("/echo?v=b")
				get("/open?name=sib")
				return r
			})
		}()
		wg.Wait()
		return strings.Join(append(res, get("/echo?v=after")), ",")
	})

	agent.Handle(cdconc.RootDuringBody, func(_ *golem.Context[state], _ golem.Unit) string {
		done := make(chan string)
		go func() {
			done <- op("op", func() string {
				get("/open?name=entered-root")
				return get("/gate?name=root")
			})
		}()
		get("/gate?name=entered-root")
		_ = durability.GenerateIdempotencyKey()
		r := get("/echo?v=root")
		get("/open?name=root")
		return <-done + "," + r
	})

	agent.Handle(cdconc.StdlibDuringBody, func(_ *golem.Context[state], _ golem.Unit) string {
		done := make(chan string)
		go func() {
			done <- op("op", func() string {
				get("/open?name=entered-stdlib")
				return get("/gate?name=stdlib")
			})
		}()
		get("/gate?name=entered-stdlib")
		_ = time.Now()
		buf := make([]byte, 8)
		_, _ = crand.Read(buf)
		fmt.Println("root goroutine output")
		get("/open?name=stdlib")
		return <-done
	})

	agent.Handle(cdconc.SleepInBody, func(_ *golem.Context[state], _ golem.Unit) string {
		return op("op", func() string {
			time.Sleep(100 * time.Millisecond)
			return get("/echo?v=slept")
		})
	})

	agent.Handle(cdconc.SleepWithSibling, func(_ *golem.Context[state], _ golem.Unit) string {
		done := make(chan string)
		go func() {
			done <- op("op", func() string {
				time.Sleep(300 * time.Millisecond)
				return get("/echo?v=slept")
			})
		}()
		r1 := get("/echo?v=r1")
		r2 := get("/echo?v=r2")
		return <-done + "," + r1 + "," + r2
	})

	agent.Handle(cdconc.Children, func(_ *golem.Context[state], _ golem.Unit) string {
		return op("parent", func() string {
			var wg sync.WaitGroup
			res := make([]string, 2)
			for i := range 2 {
				wg.Add(1)
				go func() {
					defer wg.Done()
					res[i] = get(fmt.Sprintf("/echo?v=c%d", i))
				}()
			}
			wg.Wait()
			return strings.Join(res, ",")
		})
	})

	agent.Handle(cdconc.Nested, func(_ *golem.Context[state], _ golem.Unit) string {
		return op("outer", func() string {
			var wg sync.WaitGroup
			res := make([]string, 2)
			wg.Add(2)
			go func() {
				defer wg.Done()
				res[0] = op("inner", func() string { return get("/gate?name=nested") })
			}()
			go func() {
				defer wg.Done()
				res[1] = op("inner", func() string {
					r := get("/echo?v=i1")
					get("/open?name=nested")
					return r
				})
			}()
			wg.Wait()
			return strings.Join(res, ",")
		})
	})

	agent.Handle(cdconc.Partial, func(_ *golem.Context[state], _ golem.Unit) string {
		res := make([]string, 2)
		first := make(chan struct{})
		second := make(chan struct{})
		go func() {
			res[0] = op("op", func() string { return get("/callback?payload=first") })
			close(first)
		}()
		go func() {
			res[1] = op("op", func() string { return get("/callback?payload=second") })
			close(second)
		}()
		<-first
		get("/release")
		<-second
		return strings.Join(res, ",")
	})

	agent.Handle(cdconc.Escaped, func(_ *golem.Context[state], _ golem.Unit) string {
		later := make(chan struct{})
		out := make(chan string)
		body := op("op", func() string {
			go func() {
				<-later
				out <- get("/echo?v=escaped")
			}()
			return "body"
		})
		close(later)
		return body + "," + <-out
	})

	agent.Handle(cdconc.RootSleepDuringBody, func(_ *golem.Context[state], _ golem.Unit) string {
		done := make(chan string)
		go func() {
			done <- op("op", func() string {
				return get("/echo?v=b1") + get("/echo?v=b2") + get("/echo?v=b3")
			})
		}()
		time.Sleep(200 * time.Millisecond)
		r := get("/echo?v=root")
		return <-done + "," + r
	})

	agent.Handle(cdconc.GcInBody, func(_ *golem.Context[state], _ golem.Unit) string {
		done := make(chan string)
		go func() {
			done <- op("op", func() string {
				var sum int
				for i := range 64 {
					buf := make([]byte, 1<<20)
					buf[i] = byte(i)
					sum += int(buf[i])
				}
				return get(fmt.Sprintf("/echo?v=gc%d", sum))
			})
		}()
		b := <-done
		return b + "," + get("/echo?v=root") + get("/echo?v=root2")
	})

	agent.Handle(cdconc.GcAfterBody, func(_ *golem.Context[state], _ golem.Unit) string {
		return allocateAroundBody()
	})

	agent.Handle(cdconc.BodyClockRootTimer, func(_ *golem.Context[state], _ golem.Unit) string {
		return spinPastRootTimer()
	})

	agent.Handle(cdconc.RandAfterBody, func(_ *golem.Context[state], _ golem.Unit) string {
		return drawAfterBody()
	})

	agent.Handle(cdconc.Remembered, func(_ *golem.Context[state], _ golem.Unit) string {
		return remembered
	})

	agent.Handle(cdconc.BodyTimerFirst, func(_ *golem.Context[state], _ golem.Unit) string {
		done := make(chan string)
		go func() {
			time.Sleep(60 * time.Millisecond)
			done <- get("/echo?v=r")
		}()
		b := op("op", func() string {
			time.Sleep(5 * time.Millisecond)
			return get("/echo?v=b")
		})
		return b + "," + <-done
	})

	agent.Handle(cdconc.SiblingTimers, func(_ *golem.Context[state], _ golem.Unit) string {
		var wg sync.WaitGroup
		res := make([]string, 2)
		for i, d := range []time.Duration{60 * time.Millisecond, 5 * time.Millisecond} {
			wg.Add(1)
			go func() {
				defer wg.Done()
				name := fmt.Sprintf("s%d", i)
				res[i] = op(name, func() string {
					time.Sleep(d)
					return get("/echo?v=" + name)
				})
			}()
		}
		wg.Wait()
		return strings.Join(res, ",")
	})

	agent.Handle(cdconc.AfterFuncInBody, func(_ *golem.Context[state], _ golem.Unit) string {
		b := op("op", func() string {
			ch := make(chan string, 1)
			time.AfterFunc(5*time.Millisecond, func() { ch <- get("/echo?v=af") })
			return <-ch
		})
		return b + "," + get("/echo?v=r")
	})

	agent.Handle(cdconc.TimerOutlivesBody, func(_ *golem.Context[state], _ golem.Unit) string {
		b := op("op", func() string {
			time.AfterFunc(time.Hour, func() { get("/echo?v=late") })
			t := time.NewTimer(time.Hour)
			go func() {
				select {
				case <-t.C:
				default:
				}
			}()
			return get("/echo?v=b")
		})
		time.Sleep(10 * time.Millisecond)
		return b + "," + get("/echo?v=r")
	})

	agent.Handle(cdconc.Echo, func(_ *golem.Context[state], _ golem.Unit) string {
		return get("/echo?v=echo")
	})
}

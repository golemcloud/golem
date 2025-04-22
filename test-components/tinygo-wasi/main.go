package main

import (
	"fmt"
	"math/rand"
	"os"
	"time"

	"github.com/golemcloud/golem-go/std"
	tinygowasi "golem.com/tinygo_wasi/binding/golem/it/tinygo-wasi"
)

func Example1(s string) int32 {
	std.Init(std.Packages{Os: true})

	fmt.Println(s)

	s1 := rand.NewSource(time.Now().UnixNano())
	r1 := rand.New(s1)
	v1 := r1.Int31()
	currentTime := time.Now()

	fmt.Println("test", currentTime.Year(), v1)

	fmt.Printf("args: %+v", os.Args)
	fmt.Printf("env: %+v", os.Environ())

	return v1
}

func init() {
	tinygowasi.Exports.Example1 = func(s string) int32 {
		return Example1(s)
	}
}

func main() {
}

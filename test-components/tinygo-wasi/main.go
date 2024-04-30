package main

import (
	"fmt"
	// "io/ioutil"
	"math/rand"
	"time"

	"golem.com/tinygo_wasi/tinygo_wasi"
)

func init() {
	a := TinygoWasiImpl{}
	tinygo_wasi.SetTinygoWasi(a)
}

type TinygoWasiImpl struct {
}

func (e TinygoWasiImpl) Example1(s string) int32 {
	fmt.Println(s)

	s1 := rand.NewSource(time.Now().UnixNano())
	r1 := rand.New(s1)
	v1 := r1.Int31()
	currentTime := time.Now()

	fmt.Println("test", currentTime.Year(), v1)
	return v1
}

func main() {
}

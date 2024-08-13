package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"os"

	"github.com/golemcloud/golem-go/std"
	"golem.com/tinygo_wasi/binding"

	// Using a custom client until https://github.com/golemcloud/golem/issues/709 is resolved
	"golem.com/tinygo_wasi/net/http"
)

func init() {
	binding.SetBinding(&Impl{})
}

type Impl struct {
}

type ExampleRequest struct {
	Name     string
	Amount   uint32
	Comments []string
}

type ExampleResponse struct {
	Percentage float64
	Message    string
}

func (i *Impl) Example1(_ string) string {
	std.Init(std.Modules{
		Os:   true,
		Http: true,
	})

	port := os.Getenv("PORT")

	if port == "" {
		panic("missing or empty PORT env var")
	}

	postBody, _ := json.Marshal(ExampleRequest{
		Name:     "Something",
		Amount:   42,
		Comments: []string{"Hello", "World"},
	})
	resp, err := http.Post(fmt.Sprintf("http://localhost:%s/post-example", port), "application/json", bytes.NewBuffer(postBody))
	if err != nil {
		return fmt.Sprintln(err)
	}
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return fmt.Sprintln(err)
	}

	var response ExampleResponse
	err = json.Unmarshal(body, &response)
	if err != nil {
		return fmt.Sprintln(err)
	}
	return fmt.Sprintf("%d percentage: %f, message: %s", resp.StatusCode, response.Percentage, response.Message)
}

func main() {
}

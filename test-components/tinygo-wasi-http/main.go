package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"

	tinygowasihttp "golem.com/tinygo_wasi_http/binding/golem/it/tinygo-wasi-http"

	"github.com/golemcloud/golem-go/std"
)

func init() {
	tinygowasihttp.Exports.Example1 = Example1
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

func Example1(_ string) string {
	std.Init(std.Packages{
		Os:      true,
		NetHttp: true,
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

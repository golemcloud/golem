package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"golem.com/tinygo_wasi/roundtrip"
	"golem.com/tinygo_wasi/tinygo_wasi"
	"io"
	"net/http"
	"os"
)

func init() {
	a := TinygoWasiImpl{}
	tinygo_wasi.SetTinygoWasi(a)
}

type TinygoWasiImpl struct {
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

func (e TinygoWasiImpl) Example1(s string) string {
	http.DefaultClient.Transport = roundtrip.WasiHttpTransport{}

	port := "9999"
	if p := os.Getenv("PORT"); p != "" {
		port = p
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

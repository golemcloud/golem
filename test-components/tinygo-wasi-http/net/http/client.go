package http

import (
	"io"
	stdhttp "net/http"
	"net/url"
	"strings"

	"github.com/golemcloud/golem-go/net/http"
)

// Using a custom client until this https://github.com/golemcloud/golem/issues/709 is resolved

var transport stdhttp.RoundTripper = &http.WasiHttpTransport{}

type Client struct {
	stdhttp.Client
}

var DefaultClient = &Client{}

func Get(url string) (resp *stdhttp.Response, err error) {
	return DefaultClient.Get(url)
}

func (c *Client) Get(url string) (resp *stdhttp.Response, err error) {
	req, err := stdhttp.NewRequest("GET", url, nil)
	if err != nil {
		return nil, err
	}
	return c.Do(req)
}

func (c *Client) Do(req *stdhttp.Request) (*stdhttp.Response, error) {
	return transport.RoundTrip(req)
}

func Post(url, contentType string, body io.Reader) (resp *stdhttp.Response, err error) {
	return DefaultClient.Post(url, contentType, body)
}

func (c *Client) Post(url, contentType string, body io.Reader) (resp *stdhttp.Response, err error) {
	req, err := stdhttp.NewRequest("POST", url, body)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Content-Type", contentType)
	return c.Do(req)
}

func PostForm(url string, data url.Values) (resp *stdhttp.Response, err error) {
	return DefaultClient.PostForm(url, data)
}

func (c *Client) PostForm(url string, data url.Values) (resp *stdhttp.Response, err error) {
	return c.Post(url, "application/x-www-form-urlencoded", strings.NewReader(data.Encode()))
}

func Head(url string) (resp *stdhttp.Response, err error) {
	return DefaultClient.Head(url)
}

func (c *Client) Head(url string) (resp *stdhttp.Response, err error) {
	req, err := stdhttp.NewRequest("HEAD", url, nil)
	if err != nil {
		return nil, err
	}
	return c.Do(req)
}

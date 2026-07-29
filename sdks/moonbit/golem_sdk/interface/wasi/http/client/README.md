This interface defines an HTTP client for sending "outgoing" requests.

Most components are expected to import this interface to provide the
capability to send HTTP requests to arbitrary destinations on a network.

The type signature of `client.send` is the same as `handler.handle`. This
duplication is currently necessary because some Component Model tooling
(including WIT itself) is unable to represent a component importing two
instances of the same interface. A `client.send` import may be linked
directly to a `handler.handle` export to bypass the network.
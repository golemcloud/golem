# Scala tool-streaming test component

This application owns the Scala provider/caller fixture used by the worker executor tool-streaming
tests. It depends on the `0.0.0-SNAPSHOT` Scala SDK published from `../../sdks/scala`.

Build and copy the release artifact through the shared test-component build infrastructure with:

```shell
(cd .. && ./build-components.sh scala)
```

Always build through the Golem CLI. The application must continue to produce
`../golem_it_tool_streaming_scala.wasm` and to expose the `scala:examples` component, the
`scala-streaming` tool, and `ScalaToolStreamingCaller`.

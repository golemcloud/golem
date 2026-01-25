# Need MCP server?

Golem CLI comes with http streamable mcp server, enables you to interact with all golem commands using any agent such as Claude Caude or Openai Codex.

# What are available tools?
All relevant Golem CLI commands are available as tools, refer to this to see them in action: https://youtu.be/t5dnCSYQg_0

# What are available resources?
Golem MCP server will provide all existing manifests (yaml or yml) in the current working directory (where the mcp launches) as resources, you can then read such manifest from such llm agent using just single prompt like: what's inside @suchmanifest (drop down menu will appear, just when type @), refer to this video: https://youtu.be/95BXexeZjj4  


---

# Start the MCP server

Try to build the package with features enabled
```bash
cargo build -p golem-cli --release --features "server-commands"
```
then start the binary

```bash
./target/release/golem-cli --serve --serve-port 1232
```

# Need MCP client?
Golem also comes with basic http streamable mcp client, enables you to interact with the Golem MCP server if you want standlaone client or depends of such use case.

# Start the MCP server

Try to make the ports identical (edit tests/mcp-client/src/main.rs to match the server port), then build the package
```bash
cargo build -p golem-mcp-client --release
```
then start the binary

```bash
./target/release/golem-mcp-client
```
There is a video for the e2e mcp server/client testing: https://youtu.be/ONUJ6BOyHDI





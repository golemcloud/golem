///|
test "valid agent with mount and endpoint" {
  let content =
    #|#derive.agent
    #|#derive.mount("/counters/{name}")
    #|pub(all) struct Counter {
    #|  name : String
    #|  mut value : UInt64
    #|}
    #|
    #|pub fn Counter::new(name : String) -> Counter {
    #|  { name, value: 0 }
    #|}
    #|
    #|#derive.endpoint(get="/value")
    #|pub fn Counter::get_value(self : Self) -> UInt64 {
    #|  self.value
    #|}
  let agents = @lib.parse_agents([("counter.mbt", content)])
  @lib.validate_http_routes(agents)
}

///|
test "error: catch-all in mount" {
  let content =
    #|#derive.agent
    #|#derive.mount("/files/{*rest}")
    #|pub(all) struct FileAgent {
    #|  rest : String
    #|}
    #|
    #|pub fn FileAgent::new(rest : String) -> FileAgent {
    #|  { rest }
    #|}
  let result : Result[_, _] = try? ({
    let agents = @lib.parse_agents([("file.mbt", content)])
    @lib.validate_http_routes(agents)
  })
  inspect(
    result,
    content=(
      #|Err(AgentParseError("Mount path for agent 'FileAgent' cannot contain catch-all variable 'rest'"))
    ),
  )
}

///|
test "error: mount variable not in constructor params" {
  let content =
    #|#derive.agent
    #|#derive.mount("/items/{id}")
    #|pub(all) struct ItemAgent {
    #|  name : String
    #|}
    #|
    #|pub fn ItemAgent::new(name : String) -> ItemAgent {
    #|  { name }
    #|}
  let result : Result[_, _] = try? ({
    let agents = @lib.parse_agents([("item.mbt", content)])
    @lib.validate_http_routes(agents)
  })
  inspect(
    result,
    content=(
      #|Err(AgentParseError("Mount path variable 'id' not found in constructor params of agent 'ItemAgent'"))
    ),
  )
}

///|
test "error: constructor param not bound by mount variable" {
  let content =
    #|#derive.agent
    #|#derive.mount("/items/{name}")
    #|pub(all) struct ItemAgent {
    #|  name : String
    #|  category : String
    #|}
    #|
    #|pub fn ItemAgent::new(name : String, category : String) -> ItemAgent {
    #|  { name, category }
    #|}
  let result : Result[_, _] = try? ({
    let agents = @lib.parse_agents([("item.mbt", content)])
    @lib.validate_http_routes(agents)
  })
  inspect(
    result,
    content=(
      #|Err(AgentParseError("Constructor param 'category' of agent 'ItemAgent' is not bound by any mount path variable"))
    ),
  )
}

///|
test "error: method has endpoint but agent has no mount" {
  let content =
    #|#derive.agent
    #|pub(all) struct Counter {
    #|  name : String
    #|  mut value : UInt64
    #|}
    #|
    #|pub fn Counter::new(name : String) -> Counter {
    #|  { name, value: 0 }
    #|}
    #|
    #|#derive.endpoint(get="/value")
    #|pub fn Counter::get_value(self : Self) -> UInt64 {
    #|  self.value
    #|}
  let result : Result[_, _] = try? ({
    let agents = @lib.parse_agents([("counter.mbt", content)])
    @lib.validate_http_routes(agents)
  })
  inspect(
    result,
    content=(
      #|Err(AgentParseError("Method 'get_value' has #derive.endpoint but agent 'Counter' has no #derive.mount"))
    ),
  )
}

///|
test "error: endpoint variable not in method params" {
  let content =
    #|#derive.agent
    #|#derive.mount("/items/{name}")
    #|pub(all) struct ItemAgent {
    #|  name : String
    #|}
    #|
    #|pub fn ItemAgent::new(name : String) -> ItemAgent {
    #|  { name }
    #|}
    #|
    #|#derive.endpoint(get="/detail/{id}")
    #|pub fn ItemAgent::get_detail(self : Self) -> String {
    #|  self.name
    #|}
  let result : Result[_, _] = try? ({
    let agents = @lib.parse_agents([("item.mbt", content)])
    @lib.validate_http_routes(agents)
  })
  inspect(
    result,
    content=(
      #|Err(AgentParseError("Endpoint variable 'id' not found in params of method 'get_detail' on agent 'ItemAgent'"))
    ),
  )
}

///|
test "error: endpoint variable binds to Principal param" {
  let content =
    #|#derive.agent
    #|#derive.mount("/items/{name}")
    #|pub(all) struct ItemAgent {
    #|  name : String
    #|}
    #|
    #|pub fn ItemAgent::new(name : String) -> ItemAgent {
    #|  { name }
    #|}
    #|
    #|#derive.endpoint(get="/by-user/{user}")
    #|pub fn ItemAgent::get_by_user(self : Self, user : @common.Principal) -> String {
    #|  self.name
    #|}
  let result : Result[_, _] = try? ({
    let agents = @lib.parse_agents([("item.mbt", content)])
    @lib.validate_http_routes(agents)
  })
  inspect(
    result,
    content=(
      #|Err(AgentParseError("Endpoint variable 'user' in method 'get_by_user' on agent 'ItemAgent' binds to a Principal param which is not HTTP-safe"))
    ),
  )
}

///|
test "error: duplicate header name" {
  let content =
    #|#derive.agent
    #|#derive.mount("/items/{name}")
    #|pub(all) struct ItemAgent {
    #|  name : String
    #|}
    #|
    #|pub fn ItemAgent::new(name : String) -> ItemAgent {
    #|  { name }
    #|}
    #|
    #|#derive.endpoint(get="/info")
    #|#derive.endpoint_header("X-Token", "token")
    #|#derive.endpoint_header("x-token", "token2")
    #|pub fn ItemAgent::get_info(self : Self, token : String, token2 : String) -> String {
    #|  self.name
    #|}
  let result : Result[_, _] = try? ({
    let agents = @lib.parse_agents([("item.mbt", content)])
    @lib.validate_http_routes(agents)
  })
  inspect(
    result,
    content=(
      #|Err(AgentParseError("Duplicate header name 'x-token' in endpoint of method 'get_info' on agent 'ItemAgent'"))
    ),
  )
}

///|
test "error: duplicate param binding in path and query" {
  let content =
    #|#derive.agent
    #|#derive.mount("/items/{name}")
    #|pub(all) struct ItemAgent {
    #|  name : String
    #|}
    #|
    #|pub fn ItemAgent::new(name : String) -> ItemAgent {
    #|  { name }
    #|}
    #|
    #|#derive.endpoint(get="/search/{query}?q={query}")
    #|pub fn ItemAgent::search(self : Self, query : String) -> String {
    #|  self.name
    #|}
  let result : Result[_, _] = try? ({
    let agents = @lib.parse_agents([("item.mbt", content)])
    @lib.validate_http_routes(agents)
  })
  inspect(
    result,
    content=(
      #|Err(AgentParseError("Parameter 'query' of method 'search' on agent 'ItemAgent' is bound by multiple endpoint variables"))
    ),
  )
}

///|
test "valid: agent without any HTTP annotations" {
  let content =
    #|#derive.agent
    #|pub(all) struct SimpleAgent {
    #|  name : String
    #|}
    #|
    #|pub fn SimpleAgent::new(name : String) -> SimpleAgent {
    #|  { name }
    #|}
    #|
    #|pub fn SimpleAgent::greet(self : Self) -> String {
    #|  self.name
    #|}
  let agents = @lib.parse_agents([("simple.mbt", content)])
  @lib.validate_http_routes(agents)
}

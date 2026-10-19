///|
let unstructured_agent_source : String =
  #|///| Serves documents
  #|#derive.agent
  #|pub(all) struct DocumentAgent {
  #|  name : String
  #|}
  #|
  #|pub fn DocumentAgent::new(name : String) -> DocumentAgent {
  #|  { name, }
  #|}
  #|
  #|///| Summarize a text
  #|pub fn DocumentAgent::summarize(self : Self, input : @schema.UnstructuredText[@schema.AnyLanguage]) -> @schema.UnstructuredText[Languages] {
  #|  ...
  #|}
  #|
  #|///| Store a binary
  #|pub fn DocumentAgent::store(self : Self, input : @schema.UnstructuredBinary[ImageTypes]) -> Unit {
  #|  ...
  #|}
  #|
  #|///| Optional raw body
  #|pub fn DocumentAgent::find(self : Self, key : String) -> @schema.UnstructuredBinary[@schema.AnyMimeType]? {
  #|  ...
  #|}
  #|
  #|///| Fallible raw body
  #|pub fn DocumentAgent::render(self : Self) -> Result[@schema.UnstructuredBinary[ImageTypes], String] {
  #|  ...
  #|}
  #|
  #|///| Fallible optional raw body
  #|pub fn DocumentAgent::lookup(self : Self, key : String) -> Result[@schema.UnstructuredBinary[@schema.AnyMimeType]?, String] {
  #|  ...
  #|}

///|
/// Removes the formatter's layout (whitespace and trailing commas) so the
/// assertions do not depend on line wrapping.
fn without_layout(s : String) -> String {
  s
  .replace_all(old=" ", new="")
  .replace_all(old="\n", new="")
  .replace_all(old=",]", new="]")
  .replace_all(old=",)", new=")")
}

///|
test "parse_agents accepts unstructured types as parameters and nested returns" {
  let agents = parse_agents([("document.mbt", unstructured_agent_source)])
  let signatures = agents[0].methods.map(m => (m.name, m.params, m.return_type))
  debug_inspect(
    signatures,
    content=(
      #|[
      #|  (
      #|    "summarize",
      #|    [
      #|      (
      #|        "input",
      #|        Parameterized(
      #|          "@schema.UnstructuredText",
      #|          [Simple("@schema.AnyLanguage")],
      #|        ),
      #|      ),
      #|    ],
      #|    Some(Parameterized("@schema.UnstructuredText", [Simple("Languages")])),
      #|  ),
      #|  (
      #|    "store",
      #|    [
      #|      (
      #|        "input",
      #|        Parameterized("@schema.UnstructuredBinary", [Simple("ImageTypes")]),
      #|      ),
      #|    ],
      #|    None,
      #|  ),
      #|  (
      #|    "find",
      #|    [("key", Simple("String"))],
      #|    Some(
      #|      Optional(
      #|        Parameterized(
      #|          "@schema.UnstructuredBinary",
      #|          [Simple("@schema.AnyMimeType")],
      #|        ),
      #|      ),
      #|    ),
      #|  ),
      #|  (
      #|    "render",
      #|    [],
      #|    Some(
      #|      ResultType(
      #|        Parameterized("@schema.UnstructuredBinary", [Simple("ImageTypes")]),
      #|        Simple("String"),
      #|      ),
      #|    ),
      #|  ),
      #|  (
      #|    "lookup",
      #|    [("key", Simple("String"))],
      #|    Some(
      #|      ResultType(
      #|        Optional(
      #|          Parameterized(
      #|            "@schema.UnstructuredBinary",
      #|            [Simple("@schema.AnyMimeType")],
      #|          ),
      #|        ),
      #|        Simple("String"),
      #|      ),
      #|    ),
      #|  ),
      #|]
    ),
  )
}

///|
test "generate_agent_registration emits wire schemas and codecs for nested unstructured returns" {
  let agents = parse_agents([("document.mbt", unstructured_agent_source)])
  let output = without_layout(
    @formatter.impls_to_string(generate_agent_registration(agents)),
  )
  for
    schema_type in [
      "@schema.UnstructuredText[@schema.AnyLanguage]", "@schema.UnstructuredText[Languages]",
      "@schema.UnstructuredBinary[ImageTypes]", "@schema.UnstructuredBinary[@schema.AnyMimeType]?",
      "Result[@schema.UnstructuredBinary[ImageTypes],String]", "Result[@schema.UnstructuredBinary[@schema.AnyMimeType]?,String]",
    ] {
    assert_true(
      output.contains("@schema.TypeTag[\{schema_type}]"),
      msg="missing wire schema for \{schema_type}",
    )
  }
  assert_true(
    output.contains(
      "letinput:@schema.UnstructuredText[@schema.AnyLanguage]=@agents.decode_wire_param(params,0,\"DocumentAgent.summarize\")",
    ),
  )
  assert_true(
    output.contains(
      "letresult=self.lookup(key)Ok(Some(@agents.encode_wire_output(result)))",
    ),
  )
}

///|
test "generate_client_code exposes unstructured parameters and nested returns" {
  let agents = parse_agents([("document.mbt", unstructured_agent_source)])
  let output = without_layout(
    @formatter.impls_to_string(generate_client_code(agents)),
  )
  for
    signature in [
      "pubasyncfnDocumentAgentClient::summarize(self:DocumentAgentClient,input:@schema.UnstructuredText[@schema.AnyLanguage],scope_card?:@permissions.ScopeCard)->@schema.UnstructuredText[Languages]{",
      "pubasyncfnDocumentAgentClient::store(self:DocumentAgentClient,input:@schema.UnstructuredBinary[ImageTypes],scope_card?:@permissions.ScopeCard)->Unit{",
      "pubasyncfnDocumentAgentClient::find(self:DocumentAgentClient,key:String,scope_card?:@permissions.ScopeCard)->@schema.UnstructuredBinary[@schema.AnyMimeType]?{",
      "pubasyncfnDocumentAgentClient::render(self:DocumentAgentClient,scope_card?:@permissions.ScopeCard)->Result[@schema.UnstructuredBinary[ImageTypes],String]{",
      "pubasyncfnDocumentAgentClient::lookup(self:DocumentAgentClient,key:String,scope_card?:@permissions.ScopeCard)->Result[@schema.UnstructuredBinary[@schema.AnyMimeType]?,String]{",
    ] {
    assert_true(output.contains(signature), msg="missing client \{signature}")
  }
  assert_true(
    output.contains(
      "Some(tree)=>@agents.decode_wire_output(tree,\"DocumentAgent.lookup\")",
    ),
  )
}

///|
test "parse_agents does not reject unstructured types nested in containers" {
  let source =
    #|#derive.agent
    #|pub(all) struct Archive {
    #|  name : String
    #|}
    #|
    #|pub fn Archive::new(name : String) -> Archive {
    #|  { name, }
    #|}
    #|
    #|pub fn Archive::all(self : Self, items : Array[@schema.UnstructuredText[@schema.AnyLanguage]?]) -> Array[Result[@schema.UnstructuredBinary[@schema.AnyMimeType]?, String]] {
    #|  ...
    #|}
  let agents = parse_agents([("archive.mbt", source)])
  assert_eq(agents[0].methods.length(), 1)
}

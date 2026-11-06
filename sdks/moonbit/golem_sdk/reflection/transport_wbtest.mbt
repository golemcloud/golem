///|
test "a failed awaited input encode keeps argument permission cards" {
  @test_support.reset_resource_drop_counts()
  let card_a = @card.adopt(@types.PermissionCard::PermissionCard(621))
  let input = @model.SchemaValue::Record(
    @test_support.failing_card_input(card_a, 622),
  )
  @test_support.run_async(async fn() {
    let result = try? encode_awaited_input(input)
    assert_true(result is Err(Schema(_)))
  })
  assert_true(card_a.is_present())
  assert_eq(
    @test_support.resource_drop_count(@test_support.QUOTA_TOKEN_DROPS),
    1,
  )
  assert_eq(
    @test_support.resource_drop_count(@test_support.PERMISSION_CARD_DROPS),
    0,
  )
  ignore(@card.take(card_a))
}

///|
test "identity and connection facades retain constructor schema errors" {
  let invalid = @model.SchemaValue::S8(128)
  try ParsedAgentId::create("Counter", invalid) catch {
    Schema(message) =>
      assert_true(message.has_prefix("failed to encode constructor value:"))
    error => fail("expected a schema error, got \{repr(error)}")
  } noraise {
    _ => fail("expected constructor encoding to fail before identity creation")
  }
  try Transport::create("Counter", invalid, None) catch {
    Schema(message) =>
      assert_true(message.has_prefix("failed to encode constructor value:"))
    error => fail("expected a schema error, got \{repr(error)}")
  } noraise {
    _ => fail("expected constructor encoding to fail before connection")
  }
}

///|
test "a failed reflected tool input encode keeps argument permission cards" {
  @test_support.reset_resource_drop_counts()
  let card_a = @card.adopt(@types.PermissionCard::PermissionCard(624))
  let input : @model.TypedSchemaValue = {
    graph: {
      defs: [],
      root: @model.schema_type(@model.SchemaTypeBody::Record([])),
    },
    value: Record(@test_support.failing_card_input(card_a, 625)),
  }
  let result : Result[_, @tool.ToolError[Unit]] = encode_reflected_input(input)
  assert_true(result is Err(_))
  assert_true(card_a.is_present())
  assert_eq(
    @test_support.resource_drop_count(@test_support.QUOTA_TOKEN_DROPS),
    1,
  )
  assert_eq(
    @test_support.resource_drop_count(@test_support.PERMISSION_CARD_DROPS),
    0,
  )
  ignore(@card.take(card_a))
}

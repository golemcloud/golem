///|
test "concrete ANY method is not the wildcard matcher" {
  assert_eq(http_method_str("ANY"), "@common.HttpMethod::Custom(\"ANY\")")
  assert_eq(http_method_str("any"), "@common.HttpMethod::Custom(\"any\")")
}

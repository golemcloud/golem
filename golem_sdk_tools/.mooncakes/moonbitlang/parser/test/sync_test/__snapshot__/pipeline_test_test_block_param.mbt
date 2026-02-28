test (x : @test.Test) {
  (x : @test.Test) |> ignore
}

test (x : @moonbitlang/core/test.Test) {
  (x : @test.Test) |> ignore
}

test "a test with parameter" (x : @test.Test) {
  (x : @test.Test) |> ignore
}

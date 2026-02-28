# pretty printer

A declarative-style pretty printer engine, which includes printers for built-in 
types such as `Array`, `Map`, and `Json`.

# Usage

## Print Value

Use `render` to pretty print any type implemented `Pretty` trait.

```moonbit
let map = {
  "name": ["John", "Mike"], "age": ["15","18"], "id": ["11109121","2000012312"]
}
map |> @prettyprinter.render() |> println()
```

output:
```
{
  "name": ["John", "Mike"],
  "age": ["15", "18"],
  "id": ["11109121", "2000012312"]
}
```

## Implement Pretty Trait

Write declarative code to implement a printer for your type. 
See example in package `prettyprinter/example`.


---
name: moonbit-code-transform
description: Writing MoonBit source-to-source code transformations using moonbitlang/parser and moonbitlang/formatter. Use when parsing MoonBit source into AST, constructing new AST nodes, or emitting generated MoonBit code.
---

# MoonBit Code Transformations

Guide for building source-to-source code generation tools in MoonBit using
`moonbitlang/parser` to parse source into AST and `moonbitlang/formatter` to
emit generated code. Uses `golem_sdk_tools` as the reference implementation.

## Pipeline Overview

Every code transformation follows the same pattern:

```
Source (.mbt files)
  → Parse with @parser.parse_string()
  → Extract information from @syntax.Impl list
  → Construct new @syntax.Impl nodes (AST)
  → Format with @formatter.impls_to_string()
  → Write output file
```

## Required Dependencies

In `moon.mod.json`:
```json
{
  "deps": {
    "moonbitlang/parser": "0.1.16",
    "moonbitlang/formatter": "..."
  }
}
```

In the library package `moon.pkg` (where AST construction happens):
```
import {
  "moonbitlang/core/list",
  "moonbitlang/parser",
  "moonbitlang/parser/syntax",
  "moonbitlang/parser/basic",
}

import {
  "moonbitlang/formatter",
} for "test"
```

In the CLI/main package `moon.pkg` (where formatting and file I/O happen):
```
import {
  "moonbitlang/formatter",
  "moonbitlang/x/fs",
  "your/module/lib" @lib,
}
```

**Key separation**: The library package constructs `@list.List[@syntax.Impl]`.
The CLI package calls `@formatter.impls_to_string(impls)` and writes the result.
Tests in the library import `moonbitlang/formatter` via `for "test"` to verify
output without depending on it at runtime.

## Parsing Source Files

Use `@parser.parse_string()` to parse MoonBit source into a list of top-level
AST items:

```mbt nocheck
let (impls, _reports) = @parser.parse_string(content)
// impls : @list.List[@syntax.Impl]
// _reports : Array[...] — diagnostics, usually ignored for codegen
```

### Walking the AST to Extract Information

Pattern-match on `@syntax.Impl` variants to find declarations:

```mbt nocheck
impls.each(fn(impl_) {
  match impl_ {
    // Function declarations
    TopFuncDef(fun_decl~, ..) => {
      // fun_decl : @syntax.FunDecl
      let name = fun_decl.name.name          // String
      let vis = fun_decl.vis                  // @syntax.Visibility
      let params = fun_decl.decl_params       // Option[@list.List[@syntax.Parameter]]
      let ret = fun_decl.return_type          // Option[@syntax.Type]
      let doc = fun_decl.doc                  // @syntax.DocString
      let type_name = fun_decl.type_name      // Option[@syntax.TypeName] (for methods)
      ...
    }
    // Type definitions (struct, enum)
    TopTypeDef(type_decl) => {
      let name = type_decl.tycon              // String
      let attrs = type_decl.attrs             // @list.List[@syntax.Attribute]
      let doc = type_decl.doc                 // @syntax.DocString
      ...
    }
    _ => ()
  }
})
```

### Checking Attributes (Annotations)

Attributes like `#derive.agent` are accessed via `type_decl.attrs`:

```mbt nocheck
let mut is_agent = false
type_decl.attrs.each(fn(attr) {
  if attr.raw == "#derive.agent" {
    is_agent = true
  }
})
```

### Extracting Function Parameters

Parameters come as `@list.List[@syntax.Parameter]`:

```mbt nocheck
match fun_decl.decl_params {
  Some(ps) =>
    ps.each(fn(p) {
      match p {
        Positional(binder~, ty=Some(ty)) | Labelled(binder~, ty=Some(ty)) => {
          let name = binder.name    // String
          // ty : @syntax.Type — process recursively
        }
        _ => ()
      }
    })
  None => ()
}
```

### Processing Types Recursively

`@syntax.Type` is an enum with variants like `Name`, `Option`, `Tuple`, etc.:

```mbt nocheck
fn process_type(ty : @syntax.Type) -> MyTypeRepr {
  match ty {
    Option(ty~, ..) => MyOptional(process_type(ty))
    Name(constr_id~, tys~, ..) =>
      match constr_id.id {
        Ident(name~) =>
          if name == "Array" {
            match tys {
              More(inner, ..) => MyList(process_type(inner))
              Empty => ... // error
            }
          } else if tys.is_empty() {
            MySimple(name)
          } else { ... }
        Dot(pkg~, id~) => MyQualified(pkg, id)
      }
    _ => ... // unsupported
  }
}
```

## Constructing AST Nodes

All AST nodes require a `loc` field. For generated code, use dummy locations:

```mbt nocheck
let dummy_pos : @basic.Position = { fname: "", lnum: 0, bol: 0, cnum: 0 }
let dummy_loc : @basic.Location = { start: dummy_pos, end: dummy_pos }
```

### The `@list.List` Pattern

The AST uses `@list.List[T]` (immutable linked lists), not `Array[T]`.
Build with `Array` first, then convert:

```mbt nocheck
fn[T] to_list(arr : Array[T]) -> @list.List[T] {
  @list.List::from_array(arr)
}
```

### DSL Helper Functions

Create a set of small helper functions to make AST construction readable.
Organize by category. The reference implementation is in
`golem_sdk_tools/lib/ast_helpers.mbt`.

#### Primitives

```mbt nocheck
fn make_binder(name : String) -> @syntax.Binder {
  @syntax.Binder::{ name, loc: dummy_loc }
}

fn make_label(name : String) -> @syntax.Label {
  @syntax.Label::{ name, loc: dummy_loc }
}
```

#### Type Constructors

```mbt nocheck
// Simple type: String, Int, MyStruct, etc.
fn make_type(name : String) -> @syntax.Type {
  @syntax.Type::Name(
    constr_id=@syntax.ConstrId::{
      id: @syntax.LongIdent::Ident(name~),
      loc: dummy_loc,
    },
    tys=to_list([]),
    loc=dummy_loc,
  )
}

// Generic type: Array[T], Result[T, E], etc.
fn make_parameterized_type(
  name : String,
  type_args : Array[@syntax.Type],
) -> @syntax.Type {
  @syntax.Type::Name(
    constr_id=@syntax.ConstrId::{
      id: @syntax.LongIdent::Ident(name~),
      loc: dummy_loc,
    },
    tys=to_list(type_args),
    loc=dummy_loc,
  )
}

// Option type: T?
fn make_option_type(inner : @syntax.Type) -> @syntax.Type {
  @syntax.Type::Option(ty=inner, loc=dummy_loc, question_loc=dummy_loc)
}
```

#### Literal Expressions

```mbt nocheck
fn make_string_expr(s : String) -> @syntax.Expr {
  @syntax.Expr::Constant(c=@syntax.Constant::String(s), loc=dummy_loc)
}

fn make_int_expr(n : Int) -> @syntax.Expr {
  @syntax.Expr::Constant(c=@syntax.Constant::Int(n.to_string()), loc=dummy_loc)
}

fn make_bool_expr(b : Bool) -> @syntax.Expr {
  @syntax.Expr::Constant(c=@syntax.Constant::Bool(b), loc=dummy_loc)
}
```

Other numeric types use their respective `@syntax.Constant` variants:
`UInt("0")`, `Int64("0")`, `UInt64("0")`, `Float("0.0")`, `Double("0.0")`,
`Byte("\\x00")`, `Char('a')`.

#### Identifier Expressions

```mbt nocheck
// Simple identifier: foo
fn make_ident_expr(name : String) -> @syntax.Expr {
  @syntax.Expr::Ident(
    id=@syntax.Var::{ name: @syntax.LongIdent::Ident(name~), loc: dummy_loc },
    loc=dummy_loc,
  )
}

// Qualified identifier: @pkg.foo
fn make_qualified_expr(pkg : String, id : String) -> @syntax.Expr {
  @syntax.Expr::Ident(
    id=@syntax.Var::{ name: @syntax.LongIdent::Dot(pkg~, id~), loc: dummy_loc },
    loc=dummy_loc,
  )
}

// Method reference: TypeName::method_name
fn make_method_ref(type_name : String, method_name : String) -> @syntax.Expr {
  @syntax.Expr::Method(
    type_name=@syntax.TypeName::{
      name: @syntax.LongIdent::Ident(name=type_name),
      is_object: false,
      loc: dummy_loc,
    },
    method_name=make_label(method_name),
    loc=dummy_loc,
  )
}
```

#### Constructor Expressions

```mbt nocheck
// Enum variant without args: None, Ok, Err, etc.
fn make_constr_no_args(name : String) -> @syntax.Expr {
  @syntax.Expr::Constr(
    constr=@syntax.Constructor::{
      name: @syntax.ConstrName::{ name, loc: dummy_loc },
      extra_info: @syntax.ConstructorExtraInfo::NoExtraInfo,
      loc: dummy_loc,
    },
    loc=dummy_loc,
  )
}

// Qualified enum variant: @pkg.Type::Variant
fn make_qualified_constr(
  pkg : String,
  type_name : String,
  variant : String,
) -> @syntax.Expr {
  @syntax.Expr::Constr(
    constr=@syntax.Constructor::{
      name: @syntax.ConstrName::{ name: variant, loc: dummy_loc },
      extra_info: @syntax.ConstructorExtraInfo::TypeName(@syntax.TypeName::{
        name: @syntax.LongIdent::Dot(pkg~, id=type_name),
        is_object: false,
        loc: dummy_loc,
      }),
      loc: dummy_loc,
    },
    loc=dummy_loc,
  )
}
```

#### Function Application

```mbt nocheck
fn make_positional_arg(expr : @syntax.Expr) -> @syntax.Argument {
  @syntax.Argument::{ value: expr, kind: @syntax.ArgumentKind::Positional }
}

// f(arg1, arg2, ...)
fn make_apply(func : @syntax.Expr, args : Array[@syntax.Expr]) -> @syntax.Expr {
  @syntax.Expr::Apply(
    func~,
    args=to_list(args.map(make_positional_arg)),
    attr=@syntax.ApplyAttr::NoAttr,
    loc=dummy_loc,
  )
}

// self.method(arg1, arg2, ...)
fn make_dot_apply(
  self_ : @syntax.Expr,
  method_name : String,
  args : Array[@syntax.Expr],
) -> @syntax.Expr {
  @syntax.Expr::DotApply(
    self=self_,
    method_name=make_label(method_name),
    args=to_list(args.map(make_positional_arg)),
    return_self=false,
    attr=@syntax.ApplyAttr::NoAttr,
    loc=dummy_loc,
  )
}

// lhs op rhs (e.g., a + b, x == y)
fn make_infix(
  op : String,
  lhs : @syntax.Expr,
  rhs : @syntax.Expr,
) -> @syntax.Expr {
  @syntax.Expr::Infix(
    op=@syntax.Var::{ name: @syntax.LongIdent::Ident(name=op), loc: dummy_loc },
    lhs~,
    rhs~,
    loc=dummy_loc,
  )
}
```

#### Collections and Composites

```mbt nocheck
// Record literal: { field1: expr1, field2: expr2, ... }
fn make_record(fields : Array[@syntax.FieldDef]) -> @syntax.Expr {
  @syntax.Expr::Record(
    type_name=None,
    fields=to_list(fields),
    trailing=@syntax.TrailingMark::Comma,
    loc=dummy_loc,
  )
}

fn make_field(name : String, expr : @syntax.Expr) -> @syntax.FieldDef {
  @syntax.FieldDef::{
    label: make_label(name),
    expr,
    is_pun: false,
    loc: dummy_loc,
  }
}

// Array literal: [elem1, elem2, ...]
fn make_array(elems : Array[@syntax.Expr]) -> @syntax.Expr {
  @syntax.Expr::Array(exprs=to_list(elems), loc=dummy_loc)
}

// Tuple: (a, b, c)
fn make_tuple(elems : Array[@syntax.Expr]) -> @syntax.Expr {
  @syntax.Expr::Tuple(exprs=to_list(elems), loc=dummy_loc)
}

// Array indexing: arr[idx]
fn make_array_get(array : @syntax.Expr, index : @syntax.Expr) -> @syntax.Expr {
  @syntax.Expr::ArrayGet(array~, index~, loc=dummy_loc)
}

// Type constraint: (expr : Type)
fn make_constraint_expr(expr : @syntax.Expr, ty : @syntax.Type) -> @syntax.Expr {
  @syntax.Expr::Constraint(expr~, ty~, loc=dummy_loc)
}
```

#### Control Flow

```mbt nocheck
// let name = expr; body
fn make_let(
  name : String,
  expr : @syntax.Expr,
  body : @syntax.Expr,
) -> @syntax.Expr {
  @syntax.Expr::Let(
    pattern=@syntax.Pattern::Var(make_binder(name)),
    expr~,
    body~,
    loc=dummy_loc,
  )
}

// let name : Type = expr; body
fn make_let_typed(
  name : String,
  ty : @syntax.Type,
  expr : @syntax.Expr,
  body : @syntax.Expr,
) -> @syntax.Expr {
  @syntax.Expr::Let(
    pattern=@syntax.Pattern::Constraint(
      pat=@syntax.Pattern::Var(make_binder(name)),
      ty~,
      loc=dummy_loc,
    ),
    expr~,
    body~,
    loc=dummy_loc,
  )
}

// raise expr
fn make_raise(err_value : @syntax.Expr) -> @syntax.Expr {
  @syntax.Expr::Raise(err_value~, loc=dummy_loc)
}

// guard cond else { otherwise }; body
fn make_guard_else(
  cond : @syntax.Expr,
  otherwise : @syntax.Expr,
  body : @syntax.Expr,
) -> @syntax.Expr {
  @syntax.Expr::Guard(cond~, otherwise=Some(otherwise), body~, loc=dummy_loc)
}

// try { body } catch { cases... }
fn make_try_catch(
  body : @syntax.Expr,
  catch_cases : Array[@syntax.Case],
) -> @syntax.Expr {
  @syntax.Expr::Try(
    body~,
    catch_=to_list(catch_cases),
    catch_all=false,
    try_else=None,
    has_try=true,
    try_loc=dummy_loc,
    catch_loc=dummy_loc,
    else_loc=dummy_loc,
    loc=dummy_loc,
  )
}

fn make_case(pattern : @syntax.Pattern, body : @syntax.Expr) -> @syntax.Case {
  @syntax.Case::{ pattern, body, guard_: None }
}

// fn(p1, p2) { body }
fn make_lambda(
  param_names : Array[String],
  body : @syntax.Expr,
) -> @syntax.Expr {
  let parameters = param_names.map(fn(name) {
    @syntax.Parameter::Positional(binder=make_binder(name), ty=None)
  })
  @syntax.Expr::Function(
    func=@syntax.Func::{
      body,
      error_type: @syntax.ErrorType::NoErrorType,
      has_error: None,
      is_async: None,
      kind: @syntax.FnKind::Lambda,
      loc: dummy_loc,
      parameters: to_list(parameters),
      params_loc: dummy_loc,
      return_type: None,
    },
    loc=dummy_loc,
  )
}
```

#### Sequences and Blocks

```mbt nocheck
// Handles both single expression and multi-expression sequences
fn make_sequence(exprs : Array[@syntax.Expr]) -> @syntax.Expr {
  if exprs.length() == 1 {
    exprs[0]
  } else {
    let n = exprs.length()
    let init : Array[@syntax.Expr] = []
    for i in 0..<(n - 1) {
      init.push(exprs[i])
    }
    @syntax.Expr::Sequence(
      exprs=to_list(init),
      last_expr=exprs[n - 1],
      loc=dummy_loc,
    )
  }
}
```

**Important**: `@syntax.Expr::Sequence` separates the last expression from the
rest. For a single expression, just return it directly — no `Sequence` wrapper.

#### Top-Level Nodes

```mbt nocheck
// fn init { body }
fn make_init_block(body : @syntax.Expr) -> @syntax.Impl {
  @syntax.Impl::TopExpr(
    expr=body,
    is_main=false,
    local_types=to_list([]),
    is_async=None,
    loc=dummy_loc,
  )
}

// Typed parameter for fn declarations
fn make_param(name : String, type_name : String) -> @syntax.Parameter {
  @syntax.Parameter::Positional(
    binder=make_binder(name),
    ty=Some(make_type(type_name)),
  )
}

// pub fn name(params...) -> ReturnType { body }
fn make_top_fn(
  name : String,
  params : Array[@syntax.Parameter],
  return_type : @syntax.Type,
  body : @syntax.Expr,
) -> @syntax.Impl {
  @syntax.Impl::TopFuncDef(
    fun_decl=@syntax.FunDecl::{
      type_name: None,
      name: make_binder(name),
      has_error: None,
      is_async: None,
      decl_params: Some(to_list(params)),
      params_loc: dummy_loc,
      quantifiers: to_list([]),
      return_type: Some(return_type),
      error_type: @syntax.ErrorType::NoErrorType,
      vis: @syntax.Visibility::Pub(attr=None, loc=dummy_loc),
      attrs: to_list([]),
      doc: @syntax.DocString::empty(),
    },
    decl_body=@syntax.DeclBody::DeclBody(
      local_types=to_list([]),
      expr=body,
    ),
    loc=dummy_loc,
  )
}
```

## Formatting Output

The final step converts AST nodes back to source code:

```mbt nocheck
let impls : @list.List[@syntax.Impl] = ... // constructed AST
let formatted : String = @formatter.impls_to_string(impls)
let output = "// Generated by my-tool — DO NOT EDIT!\n\n" + formatted
```

`@formatter.impls_to_string()` produces properly formatted MoonBit source with
`///|` block separators.

## Real-World Examples

### Example 1: Generating Function Re-exports

**Goal**: Parse `.mbt` source files, find `pub fn` declarations, generate
wrapper functions that delegate to the original package.

**Input**: Source file with `pub fn wasmExportSave() -> Int { 42 }`

**Output**:
```mbt nocheck
///|
pub fn wasmExportSave() -> Int {
  @gen.wasmExportSave()
}
```

**Implementation** (from `golem_sdk_tools/lib/reexports.mbt`):

```mbt nocheck
pub fn generate_reexports(
  fns : Array[FnSignature],
  gen_pkg : String,
) -> @list.List[@syntax.Impl] {
  let impls : Array[@syntax.Impl] = []
  for fn_ in fns {
    let params : Array[@syntax.Parameter] = []
    let args : Array[@syntax.Argument] = []
    for j, param_type in fn_.params {
      let name = "p\{j}"
      params.push(make_param(name, param_type))
      args.push(make_positional_arg(make_ident_expr(name)))
    }
    let call_expr = make_apply(
      make_qualified_expr(gen_pkg, fn_.gen_name),
      args.map(fn(a) { a.value }),
    )
    let fun_decl : @syntax.FunDecl = { ... }  // see source for full details
    impls.push(@syntax.Impl::TopFuncDef(fun_decl~, ...))
  }
  to_list(impls)
}
```

### Example 2: Generating Agent Registration Code

**Goal**: Find structs annotated with `#derive.agent`, extract their `::new`
constructor signatures, and generate an `fn init { ... }` block with
`register_agent(...)` calls.

**Input**: User-written agent code:
```mbt nocheck
///| A counter agent
#derive.agent
pub(all) struct Counter {
  name : String
  mut value : UInt64
}

///| Creates a new counter
pub fn Counter::new(name : String) -> Counter {
  { name, value: 0 }
}
```

**Output** (generated `golem_agents.mbt`):
```mbt nocheck
fn init {
  @agents.register_agent({
    name: "Counter",
    agent_type: { type_name: "Counter", description: "A counter agent", ... },
    construct: fn(input) {
      try {
        let elements = @extractor.extract_tuple(input)
        guard elements.length() == 1 else {
          raise @common.AgentError::InvalidInput(
            "Expected 1 elements, got " + elements.length().to_string(),
          )
        }
        let name : String = @schema.from_element_value_as(elements[0])
        Ok(Counter::new(name))
      } catch {
        e => Err(e)
      }
    },
  })
}
```

**Implementation pattern** (from `golem_sdk_tools/lib/agents_emit.mbt`):

The construct function body is built inside-out, starting with the innermost
expression (`Ok(Counter::new(args...))`) and wrapping outward:

```mbt nocheck
fn build_construct_fn(agent : AgentInfo) -> @syntax.Expr {
  // 1. Build innermost: Ok(AgentName::new(p1, p2, ...))
  let ok_call = make_apply(make_constr_no_args("Ok"), [
    make_apply(make_method_ref(agent.name, "new"), new_args),
  ])

  // 2. Wrap with let bindings (from last parameter to first)
  let mut body = ok_call
  for idx in 0..<param_count {
    let i = param_count - 1 - idx
    body = make_let_typed(name, ty, deserialize_expr, body)
  }

  // 3. Wrap with guard for parameter count check
  body = make_guard_else(length_check, raise_error, body)

  // 4. Wrap with let elements = extract_tuple(input)
  let try_body = make_let("elements", extract_call, body)

  // 5. Wrap in fn(input) { try { ... } catch { e => Err(e) } }
  make_lambda(["input"], make_try_catch(try_body, [catch_case]))
}
```

**Key insight**: Build AST expressions inside-out. `make_let(name, expr, body)`
nests — the `body` of one `let` contains the next `let`. Start with the
innermost expression and wrap outward.

## Testing Code Transformations

Use snapshot tests with `@formatter.impls_to_string()` to verify output:

```mbt nocheck
test "generate_reexports produces correct output" {
  let fns : Array[FnSignature] = [
    { gen_name: "wasmExportSave", user_name: "wasmExportSave",
      params: [], return_type: "Int" },
  ]
  let impls = generate_reexports(fns, "gen")
  let output = @formatter.impls_to_string(impls)
  inspect(
    output,
    content=(
      #|///|
      #|pub fn wasmExportSave() -> Int {
      #|  @gen.wasmExportSave()
      #|}
      #|
      #|
    ),
  )
}
```

Run `moon test --update` to auto-populate the `content=` parameter, then review
the snapshot to confirm correctness.

For the parsing side, test extraction separately:

```mbt nocheck
test "parse_agents finds simple agent struct" {
  let content =
    #|///| A counter agent
    #|#derive.agent
    #|pub(all) struct Counter {
    #|  name : String
    #|  mut value : UInt64
    #|}
    #|
    #|///| Creates a new counter
    #|pub fn Counter::new(name : String) -> Counter {
    #|  { name, value: 0 }
    #|}
  let agents = parse_agents([("counter.mbt", content)])
  inspect(agents[0].name, content="Counter")
  inspect(agents[0].constructor_params[0].1, content="Simple(\"String\")")
}
```

## Key AST Types Reference

### `@syntax.Impl` — Top-Level Items

| Variant | Purpose |
|---|---|
| `TopFuncDef(fun_decl~, decl_body~, loc~)` | Function definition (`pub fn ...`) |
| `TopExpr(expr~, is_main~, local_types~, ..)` | Init/main block (`fn init { ... }`) |
| `TopTypeDef(type_decl)` | Type definition (`struct`, `enum`) |
| `TopLetDef(...)` | Top-level `let` binding |
| `TopTest(...)` | Test block |

### `@syntax.Expr` — Expressions (Most Common Variants)

| Variant | Generates |
|---|---|
| `Constant(c~, loc~)` | Literals: `"hello"`, `42`, `true` |
| `Ident(id~, loc~)` | Identifiers: `foo`, `@pkg.bar` |
| `Method(type_name~, method_name~, loc~)` | Method refs: `Counter::new` |
| `Constr(constr~, loc~)` | Constructors: `None`, `Ok`, `@pkg.Type::Variant` |
| `Apply(func~, args~, attr~, loc~)` | Function call: `f(a, b)` |
| `DotApply(self~, method_name~, args~, ..)` | Method call: `x.foo(a)` |
| `Infix(op~, lhs~, rhs~, loc~)` | Binary op: `a + b`, `x == y` |
| `Let(pattern~, expr~, body~, loc~)` | Let binding: `let x = e; body` |
| `Array(exprs~, loc~)` | Array literal: `[a, b, c]` |
| `Tuple(exprs~, loc~)` | Tuple: `(a, b)` |
| `Record(type_name~, fields~, ..)` | Record: `{ x: 1, y: 2 }` |
| `ArrayGet(array~, index~, loc~)` | Index: `arr[i]` |
| `Constraint(expr~, ty~, loc~)` | Type annotation: `(e : T)` |
| `Guard(cond~, otherwise~, body~, loc~)` | Guard: `guard c else { ... }; body` |
| `Raise(err_value~, loc~)` | Raise error: `raise e` |
| `Try(body~, catch_~, ..)` | Try-catch: `try { ... } catch { ... }` |
| `Function(func~, loc~)` | Lambda: `fn(x) { body }` |
| `Sequence(exprs~, last_expr~, loc~)` | Multi-statement: `a; b; c` |
| `Field(record~, accessor~, loc~)` | Field access: `r.field` |

### `@syntax.Constant` — Literal Variants

| Variant | Example |
|---|---|
| `String(String)` | `"hello"` |
| `Int(String)` | `42` (note: string representation) |
| `UInt(String)` | `42U` |
| `Int64(String)` | `42L` |
| `UInt64(String)` | `42UL` |
| `Float(String)` | `1.0F` |
| `Double(String)` | `1.0` |
| `Bool(Bool)` | `true` |
| `Byte(String)` | `b'\x00'` |
| `Char(String)` | `'a'` |

### `@syntax.LongIdent` — Identifiers

| Variant | Represents |
|---|---|
| `Ident(name~)` | Simple: `foo` |
| `Dot(pkg~, id~)` | Qualified: `@pkg.foo` |

### `@syntax.FunDecl` — Function Declaration Fields

| Field | Type | Purpose |
|---|---|---|
| `name` | `Binder` | Function name |
| `type_name` | `Option[TypeName]` | `Some(T)` for `T::method` |
| `vis` | `Visibility` | `Pub(..)` or `Priv` |
| `decl_params` | `Option[List[Parameter]]` | Parameters |
| `return_type` | `Option[Type]` | Return type |
| `doc` | `DocString` | Doc comments |
| `attrs` | `List[Attribute]` | Attributes/annotations |
| `quantifiers` | `List[...]` | Type parameters |
| `error_type` | `ErrorType` | Error type annotation |

## Common Patterns

### Inside-Out Expression Building

When generating nested `let` bindings, build from the innermost expression
outward. Each `make_let` wraps the previous body:

```mbt nocheck
let mut body = final_expr
for i = params.length() - 1; i >= 0; i = i - 1 {
  body = make_let(params[i].name, params[i].init, body)
}
// body is now: let p0 = ...; let p1 = ...; final_expr
```

### Multiple Statements in a Block

Use `make_sequence` to combine multiple expressions into a block body.
For `fn init { stmt1; stmt2; ... }`:

```mbt nocheck
let calls = agents.map(fn(a) { build_register_call(a) })
make_init_block(make_sequence(calls))
```

### Generated File Convention

Always prefix generated files with a comment and write to a well-known filename:

```mbt nocheck
let output = "// Generated by my-tool — DO NOT EDIT!\n\n" + formatted
@fs.write_string_to_file("\{target_dir}/my_generated.mbt", output)
```

### CLI Entry Point Pattern

```mbt nocheck
fn main {
  let args = @env.args()
  if args.length() < 2 { println("Usage: ..."); return }
  if args[1] == "my-command" {
    run_my_command(args[2]) catch { e => println("Error: \{e}") }
  }
}

fn run_my_command(dir : String) -> Unit raise {
  // 1. Read source files
  let entries = @fs.read_dir(dir)
  let files : Array[(String, String)] = []
  for entry in entries {
    if entry.has_suffix(".mbt") && entry != "my_generated.mbt" {
      files.push((entry, @fs.read_file_to_string("\{dir}/\{entry}")))
    }
  }
  // 2. Parse & extract
  let info = @lib.parse_things(files)
  // 3. Generate AST
  let impls = @lib.generate_things(info)
  // 4. Format & write
  let formatted = @formatter.impls_to_string(impls)
  let output = "// Generated by my-tool — DO NOT EDIT!\n\n" + formatted
  @fs.write_string_to_file("\{dir}/my_generated.mbt", output)
}
```

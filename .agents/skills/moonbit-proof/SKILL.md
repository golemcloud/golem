---
name: moonbit-proof
description: Use when writing or refactoring proof-carrying code in MoonBit, especially for Why3-backed specifications, abstraction functions, representation invariants, proof assertions, recursive verified data structures, or reducing trusted proof bridges.
---

# MoonBit Proof-Carrying Code

Use this skill when the task is to write, extend, or debug verified MoonBit code.

Typical triggers:
- add contracts to executable MoonBit code
- define an abstract model or representation invariant
- verify a recursive data structure
- connect a concrete representation to a set/map model
- replace trusted proof bridges with lemmas
- debug proof failures, timeouts, or frontend lowering limits

## Goal

Write proof-carrying code, not proof-shaped comments.

That means:
- the runtime code remains executable and readable
- the proof model is explicit
- contracts talk about named predicates/functions with consistent roles
- local `proof_assert` steps explain why the implementation satisfies the model

## Naming

Prefer `model(...)` as the default name for the proof-side semantic view of a value.

Examples:
- `model(set) : Fset[Int]`
- `model(map) : Fmap[Int, Int]`
- `model(tree) : Seq[Int]`

Use a more specific name only when it is materially clearer:
- `elements(node)` when the model is literally the set of elements stored in a recursive subtree
- `domain(bitmap)` when the model is specifically the occupied index set
- `height(tree)` when it is a structural measure, not the main semantic model

Default rule:
- use `model` for the main semantic abstraction
- use specialized names like `elements`, `domain`, `height`, or `rank` for auxiliary views

## Default Structure

Split the package into two layers.

- `.mbtp`
  - model functions such as `model(x)`
  - representation invariants such as `tree_inv(x)` or `sparse_array_inv(x)`
  - named proof predicates such as `insert_pre(...)` and `insert_post(...)`
  - reusable lemmas
- `.mbt`
  - executable code
  - contracts over the proof-side predicates
  - local `proof_assert` steps after construction, branching, and loops

Use this default unless there is a strong reason not to.

## Recommended Workflow

1. Choose the abstract model first.
2. Define the smallest useful invariant.
3. State contracts with named `*_pre` / `*_post` predicates.
4. Implement the runtime code.
5. Add loop invariants for every proof-relevant loop.
6. Add local `proof_assert` steps where the solver needs help.
7. Introduce helper lemmas only after seeing actual failing VCs.
8. Shrink trusted bridges from constructors outward.

Do not start by writing a large pile of lemmas.

## Step 1: Pick the Right Abstract Model

Prefer the simplest model that matches the observable behavior.

- Membership-only structure: use a finite set.
- Key-value structure: use a finite map.
- Recursive tree/trie: use `model(node)` or, if clearer, `elements(node)`.
- Packed layout: separate domain/layout facts from semantic meaning.

Example:

```moonbit
fn model(set : HashSet) -> Fset[Int] {
  match set.root {
    None => @fset.fset_empty()
    Some(node) => model(node)
  }
}
```

If a recursive helper really denotes the element set of a subtree, this is also reasonable:

```moonbit
fn elements(node : Node) -> Fset[Int] {
  match node {
    Empty => @fset.fset_empty()
    Branch(l, x, r) => elements(l).union(elements(r)).add(x)
  }
}
```

Avoid putting the whole semantics directly into every contract.

## Step 2: Keep the Invariant Small

The invariant should mostly describe:
- shape
- bounds
- layout
- well-formedness

Semantic equalities usually belong in postconditions or lemmas, not inside `*_inv`.

Good:

```moonbit
predicate sparse_ok(sa : SparseArray) {
  sa.data.length() == count_value(sa.bitmap, 0) &&
  (∀ i : Int,
    valid_idx(i) && mem_value(sa.bitmap, i) →
      0 <= rank_value(sa.bitmap, i, 0) &&
      rank_value(sa.bitmap, i, 0) < sa.data.length())
}
```

Not good:
- putting every update theorem into the invariant
- encoding the entire semantic equality into `*_inv`

## Step 3: Use Named Postconditions

Prefer named predicates over repeating large formulas. As a default naming convention, use `*_inv`, `*_pre`, and `*_post`.

Good:

```moonbit
predicate singleton_post(res : SparseArray, idx : Int, value : Int) {
  sparse_ok(res) &&
  model(res).eq(@fmap.fmap_empty().add(idx, value))
}

pub fn singleton(idx : Int, value : Int) -> SparseArray where {
  proof_require: valid_idx(idx),
  proof_ensure: result => singleton_post(result, idx, value),
} {
  ...
}
```

This keeps contracts short and gives the solver a reusable target.

## Step 4: Put the Math in `.mbtp`

Proof-side material belongs in `.mbtp`.

Examples:

```moonbit
fn model(t : Tree) -> Fset[Int] {
  match t {
    Empty => @fset.fset_empty()
    Node(l, x, r, _) => model(l).union(model(r)).add(x)
  }
}

predicate avl(t : Tree) {
  match t {
    Empty => true
    Node(l, x, r, h) =>
      avl(l) &&
      avl(r) &&
      all_lt(model(l), x) &&
      all_gt(x, model(r)) &&
      h == 1 + max2(height(l), height(r))
  }
}
```

Keep `.mbtp` focused on:
- logic definitions
- predicates
- lemmas

Avoid filling `.mbtp` with runtime implementation details.

Two recurring helper patterns are especially useful:

1. Extensional equality hypotheses for abstract structures.

Example:

```moonbit
predicate fmap_eq_hyp(m1 : Fmap[Int, Int], m2 : Fmap[Int, Int]) {
  (∀ k : Int, m1.mem(k) == m2.mem(k)) &&
  (∀ k : Int, m1.mem(k) → m1.find(k) == m2.find(k))
}

lemma fmap_eq_intro(m1 : Fmap[Int, Int], m2 : Fmap[Int, Int]) where {
  proof_require: fmap_eq_hyp(m1, m2),
  proof_ensure: m1.eq(m2),
} {
}
```

This is often the cleanest way to finish map-refinement proofs.

2. Small transport lemmas for updates.

Examples:
- add/remove `mem` at self and other keys
- add/remove `find` at self and other keys
- set cardinality after adding/removing an absent/present element

Prefer several small transport lemmas over one giant “everything changed correctly” theorem.

## Step 5: Guide the Solver in `.mbt`

After constructing data, assert the concrete facts the solver may miss.

Example:

```moonbit
let data = FixedArray::make(1, value)
proof_assert data.length() == 1
proof_assert data[0] == value
let result = { bitmap, data }
proof_assert sparse_ok(result)
proof_assert singleton_post(result, idx, value)
result
```

Use `proof_assert`:
- after record construction
- after array writes
- after case splits
- after loop bodies establish a stronger relation

Prefer this over introducing a callable trusted wrapper function.

## Step 6: Write Loop Invariants Early

Any loop that is relevant to the proof should get invariants as soon as the loop shape stabilizes.

In practice, proof-carrying MoonBit code often relies on loops for:
- copying array prefixes or suffixes
- accumulating counts or ranks
- building a result structure incrementally
- iterating over a subtree or packed representation

Do not wait for the prover to fail before writing the obvious invariants.

Typical invariants:
- index bounds
- relationship between the accumulator and the abstract model so far
- prefix/suffix copy facts
- preservation of unchanged fields

Example:

```moonbit
for j = 0, acc = 0; j < idx; {
  let next_acc = if bitmap_mem(bitmap, j) { acc + 1 } else { acc }
  proof_assert next_acc == rank_value(bitmap, j + 1, 0)
  continue j + 1, next_acc
} nobreak {
  acc
} where {
  proof_invariant: 0 <= j,
  proof_invariant: j <= idx,
  proof_invariant: acc == rank_value(bitmap, j, 0),
}
```

For array updates, use staged invariants that match the proof shape.

Example:

```moonbit
for i = 0; i < pos; {
  new_data[i] = old_data[i]
  continue i + 1
} where {
  proof_invariant: 0 <= i,
  proof_invariant: i <= pos,
  proof_invariant: add_prefix_ok(old_data, new_data, pos, i),
}
```

Then strengthen to a second invariant after the inserted/removed element is handled.

Default rule:
- if a loop contributes to a postcondition, its invariant should mention the proof-side progress explicitly
- if a loop only mutates concrete state, the invariant should still state the concrete relation needed by the next abstraction lemma
- if the loop's final yielded value matters semantically, add `proof_yield` so the prover knows what the yielded result satisfies

Example:

```moonbit
for i = 0, acc = 0; i < xs.length(); {
  continue i + 1, acc + xs[i]
} nobreak {
  acc
} where {
  proof_invariant: 0 <= i,
  proof_invariant: i <= xs.length(),
  proof_invariant: acc == prefix_sum(xs, i),
  proof_yield: res => res == prefix_sum(xs, xs.length()),
}
```

Use `proof_yield` when the proof needs a fact about the value produced by the whole loop expression, not just the state maintained during iteration.

## Step 7: Verify the Natural API Surface

If the public API is method-oriented, verify the methods directly.

Example:

```moonbit
pub fn HashSet::contains(self : HashSet, key : Int) -> Bool where {
  proof_require: set_inv(self),
  proof_ensure: result => result == model(self).mem(key),
} {
  ...
}
```

Use top-level verified helper functions only when they improve structure or reuse, not as a workaround for method contracts.

## Step 8: Use Structural Proof Shape for Recursive Code

For recursive data structures:
- define a semantic view like `model(node)` or `elements(node)`
- define a shape invariant like `node_ok(node, level)`
- recurse structurally
- add `proof_decrease`

Example:

```moonbit
fn contains_at(node : Node, key : Int, level : Int) -> Bool where {
  proof_decrease: node,
  proof_require: node_ok(node, level),
  proof_ensure: result => result == model(node).mem(key),
} {
  match node {
    Flat(k) => key == k
    Branch(children) => ...
  }
}
```

If the solver resists tail-recursive loops in contracted functions, try structurally recursive code first.

## Step 9: For Packed or Indexed Representations, Prove Concrete Updates Before Semantic Meaning

When a representation is packed, indexed, or incrementally rebuilt, do not jump straight from low-level mutation to the final semantic theorem.

First prove concrete update facts that match the implementation structure, then connect them to the abstract model.

A common progression is:

1. basic domain or indexing facts
2. local bounds or position facts
3. concrete update facts for unchanged and changed regions
4. a full concrete-update predicate
5. the final semantic `*_post` theorem

The exact intermediate predicates depend on the implementation. Choose names that reflect the actual stages in the code.

Typical stages are:
- unchanged region
- updated region
- shifted or rebuilt region
- full concrete-update predicate
- final semantic postcondition

Example pattern:

```moonbit
predicate update_prefix_ok(before_data, after_data, upto) { ... }
predicate update_middle_ok(before_data, after_data, pos, value, upto) { ... }
predicate update_data_ok(before, key, value, after) { ... }

lemma update_model_lemma(...) where {
  proof_require: update_data_ok(...),
  proof_ensure: update_post(...),
} {
  ...
}
```

For sparse or dense-array code, a more specific ladder like `*_prefix_ok`, `*_fill_ok`, and `*_data_ok` is often effective, but treat that as one useful instance of the general technique rather than a universal template.

## Step 10: Keep Shared Shim Packages Small

If you have reusable proof imports or theories, put them in shim packages.

Typical examples:
- finite-set wrappers
- finite-map wrappers
- bitmap domain/rank/count helpers

The benefit is:
- client packages stay focused
- imports are not duplicated
- shared reasoning is easier to test for regressions

But keep shared shims minimal. Large shared lemma sets can perturb unrelated proofs.

Also account for lowering quirks:
- methods may work in contracts while static constructors do not
- a free wrapper like `fmap_mk(...)` may still be needed even if `Fmap::mk(...)` parses
- keep those wrappers in the shim package, not duplicated in every client

If a helper is only needed by one package, prefer a local lemma there rather than exporting it from a shared shim.

## Step 11: Treat Trust as Temporary

Trusted helpers are acceptable as narrow bridges, but they should not be the design endpoint.

If trust is unavoidable:
- keep preconditions concrete
- target one named predicate
- keep the mathematical statement in `.mbtp`

Good temporary bridge:

```moonbit
fn singleton_bridge(res : SparseArray, idx : Int, value : Int) -> Unit where {
  proof_axiomatized: true,
  proof_require: valid_idx(idx),
  proof_require: res.data.length() == 1,
  proof_require: res.data[0] == value,
  proof_ensure: singleton_ok(res, idx, value),
} {
  ()
}
```

Then remove trusted bridges in this order:
1. constructors
2. observers
3. update functions
4. primitive machine-word bridges

## Debugging Rule: Inspect the Actual Failure First

After a proof failure:
- run `moon prove <pkg>`
- inspect `_build/verif/<pkg>/<pkg>.proof.json`

Classify the problem before editing:
- missing arithmetic/index fact
- missing semantic bridge
- bad quantifier instantiation
- solver perturbation from a new lemma
- frontend/lowering limitation

Different causes need different fixes.

Examples:
- missing index fact → add a local `proof_assert`
- missing model bridge → add a helper lemma or predicate
- solver perturbation → move a lemma out of a shared shim
- lowering limitation → simplify the proof surface or probe a smaller reproducer

Common reproducer strategy:
- isolate the construct in a tiny probe package
- check whether `moon check` fails, `moon prove` crashes, or the VC merely times out
- only then decide whether the issue is modeling, solver guidance, or compiler lowering

## Regression Discipline

After every proof edit:

```text
moon check <pkg>
moon prove <pkg>
moon test <pkg>   # if runtime code changed
```

After editing shared proof layers, rerun dependent packages too.

Do not assume a local fix is safe globally.

## Anti-Patterns

Avoid:
- repeating raw `#proof_import` in every client package
- large inline contract formulas instead of named predicates
- changing abstraction design and solver guidance in one step
- adding many helper lemmas without checking the proof report first
- storing semantic theorems only in trusted `.mbt` functions
- verifying methods first when top-level functions would be simpler
- introducing generic abstractions too early when a monomorphic first slice will prove faster

## Minimal Checklist

Before handing off a verified MoonBit change, confirm:
- a semantic `model(...)` exists, or there is a clear reason to use a more specific name like `elements(...)`
- a `*_inv` predicate exists
- contracts mention named `*_pre` / `*_post` predicates when appropriate
- extensional equality is handled explicitly when the abstract model is a set/map
- proof-specific logic is mostly in `.mbtp`
- runtime code has local `proof_assert` where needed
- proof-relevant loops have explicit `proof_invariant`
- the trusted surface is explicit and as small as possible
- `moon check`, `moon prove`, and any needed `moon test` commands were run

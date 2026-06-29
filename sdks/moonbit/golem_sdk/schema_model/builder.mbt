// Schema-graph construction helpers.
//
// `SchemaBuilder` registers named (nominal) types into a graph using a
// reserve/commit protocol so recursive and mutually-recursive types close
// without infinite recursion: a type reserves its id before walking its body,
// so a back-reference encountered during the walk short-circuits to a `Ref`.
//
// `merge_agent_graphs` combines the per-root graphs of an agent's constructor,
// methods, and config into a single graph, deduplicating shared definitions by
// stable `TypeId` and rejecting divergent definitions of the same id.
//
// This mirrors the Rust `golem-schema` `SchemaBuilder` / `merge_agent_graphs`
// in `conversion.rs`: definitions are kept in an ordered list in first-seen /
// commit order (inner types committed before the outer type that referenced
// them) — they are never sorted.

///|
/// Builds a `SchemaGraph`'s named definitions with a reserve/commit protocol.
///
/// `reserved` holds ids that have been reserved but not yet committed (so a
/// recursive back-reference closes to a `Ref` without infinite recursion).
/// `defs` holds committed definitions in commit order.
pub struct SchemaBuilder {
  defs : Array[SchemaTypeDef]
  reserved : Map[TypeId, Unit]
}

///|
pub fn SchemaBuilder::new() -> SchemaBuilder {
  { defs: [], reserved: Map::new() }
}

///|
fn SchemaBuilder::index_of(self : SchemaBuilder, id : TypeId) -> Int? {
  for i, d in self.defs {
    if d.id == id {
      return Some(i)
    }
  }
  None
}

///|
/// Whether the given id is already reserved or committed.
pub fn SchemaBuilder::contains(self : SchemaBuilder, id : TypeId) -> Bool {
  self.reserved.contains(id) || !(self.index_of(id) is None)
}

///|
/// Reserve a slot for `id` so recursive references can close to a `Ref`.
pub fn SchemaBuilder::reserve(self : SchemaBuilder, id : TypeId) -> Unit {
  self.reserved[id] = ()
}

///|
/// Commit the body of a previously reserved (or new) `id`. If a definition with
/// this id already exists it is overwritten in place (keeping its position);
/// otherwise it is appended in commit order.
pub fn SchemaBuilder::commit(
  self : SchemaBuilder,
  id : TypeId,
  body : SchemaType,
  name : String?,
) -> Unit {
  let _ = self.reserved.remove(id)
  let def : SchemaTypeDef = { id, name, body }
  match self.index_of(id) {
    Some(i) => self.defs[i] = def
    None => self.defs.push(def)
  }
}

///|
/// A `Ref` schema type pointing at `id`.
pub fn SchemaBuilder::ref_(self : SchemaBuilder, id : TypeId) -> SchemaType {
  ignore(self)
  schema_type(Ref(id))
}

///|
/// Register a nominal type. If `id` is not yet known it is reserved, then
/// `build` is invoked to produce its body (during which recursive references to
/// `id` resolve to a `Ref`), then committed. Always returns a `Ref` to `id`.
///
/// `build` is infallible: schema construction mirrors the Rust `IntoSchema`
/// `register_in`, where the recursive walk never fails (well-formedness is
/// validated later, at `finish`/graph-validation time). Keeping it infallible
/// lets `IntoSchema::register_in` impls and generated code construct schemas
/// without threading an error effect.
pub fn SchemaBuilder::register(
  self : SchemaBuilder,
  id : TypeId,
  build : () -> SchemaType,
  name : String?,
) -> SchemaType {
  if !self.contains(id) {
    self.reserve(id)
    self.commit(id, build(), name)
  }
  self.ref_(id)
}

///|
/// Finalize the registered definitions, ensuring every reservation was
/// committed. Definitions are returned in commit order.
pub fn SchemaBuilder::finish(
  self : SchemaBuilder,
) -> Array[SchemaTypeDef] raise SchemaModelError {
  for id, _ in self.reserved {
    raise BuilderError(
      "schema builder: reserved type id '\{id}' was never committed",
    )
  }
  self.defs
}

///|
/// Build a complete graph with the given root and all registered definitions.
pub fn SchemaBuilder::build_graph(
  self : SchemaBuilder,
  root : SchemaType,
) -> SchemaGraph raise SchemaModelError {
  { defs: self.finish(), root }
}

///|
/// Merge definitions from several graphs, deduplicating by `TypeId` in
/// first-seen order and rejecting conflicting same-id bodies. Conflict
/// detection uses exact body equality (including metadata); `name` is
/// display-only and does not participate.
pub fn merge_graph_defs(
  graphs : Array[SchemaGraph],
) -> Array[SchemaTypeDef] raise SchemaModelError {
  let merged : Array[SchemaTypeDef] = []
  for g in graphs {
    for def in g.defs {
      let mut found = false
      for existing in merged {
        if existing.id == def.id {
          found = true
          if existing.body != def.body {
            raise MergeConflict(def.id)
          }
          break
        }
      }
      if !found {
        merged.push(def)
      }
    }
  }
  merged
}

///|
/// An agent-level merged graph: a shared def registry plus the roots it backs.
pub(all) struct MergedAgentGraph {
  defs : Array[SchemaTypeDef]
  roots : Array[SchemaType]
} derive(Show, Eq)

///|
/// Combine the per-root graphs of an agent (one per constructor / method input
/// / output / config root) into a single deduplicated def registry plus the
/// list of roots, in input order. Conflicting same-id definitions raise
/// `MergeConflict`.
///
/// Unlike the Rust `merge_agent_graphs` (which discards roots and relies on the
/// caller to retain them), this returns the roots so the agent layer can install
/// them into `NamedField.schema` / `OutputSchema::Single` / config decls.
pub fn merge_agent_graphs(
  graphs : Array[SchemaGraph],
) -> MergedAgentGraph raise SchemaModelError {
  { defs: merge_graph_defs(graphs), roots: graphs.map(g => g.root) }
}

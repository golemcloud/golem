import type { SchemaGraph, SchemaType, SchemaTypeBody } from "./schema-model/model.js"
import { composeSchemaGraphs } from "./schema-model/builder.js"
import { validateSchemaGraph } from "./schema-model/validation.js"

export const configName = (name: string): string => {
  const words = name
    .replace(/([A-Z])([A-Z][a-z])/g, "$1 $2")
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .split(/[ _-]+/)
    .filter(Boolean)
    .map((word) => word.toLowerCase())
  const canonical = words
    .map((word, index) => (index === 0 ? word : word[0]!.toUpperCase() + word.slice(1)))
    .join("")
  if (!/^[A-Za-z_-][A-Za-z0-9_-]*$/.test(name) || !/^[A-Za-z_-][A-Za-z0-9_-]*$/.test(canonical))
    throw new Error(`invalid config field '${name}'`)
  return canonical
}

/** Project only config metadata; the source codec still reads native properties positionally. */
export const configGraph = (source: SchemaGraph, scope: string): SchemaGraph => {
  const composed = composeSchemaGraphs([source], scope)
  const graph = { defs: composed.defs, root: composed.roots[0]! }
  const refId = (id: string) => `${id}/config`
  const visit = (type: SchemaType): SchemaType => {
    const body = type.body
    let projected: SchemaTypeBody = body
    switch (body.tag) {
      case "ref":
        projected = { ...body, id: refId(body.id) }
        break
      case "record": {
        const names = new Set<string>()
        projected = {
          ...body,
          fields: body.fields.map((field) => {
            const name = configName(field.name)
            if (names.has(name)) throw new Error(`duplicate canonical config field '${name}'`)
            names.add(name)
            return { ...field, name, body: visit(field.body) }
          }),
        }
        break
      }
      case "union":
        projected = {
          ...body,
          branches: body.branches.map((branch) => {
            const rule = branch.discriminator
            const discriminator =
              rule.tag === "field-equals"
                ? { ...rule, val: { ...rule.val, fieldName: configName(rule.val.fieldName) } }
                : rule.tag === "field-absent"
                  ? { ...rule, val: configName(rule.val) }
                  : rule
            return { ...branch, discriminator, body: visit(branch.body) }
          }),
        }
        break
      case "variant":
        projected = {
          ...body,
          cases: body.cases.map((entry) => ({
            ...entry,
            payload: entry.payload && visit(entry.payload),
          })),
        }
        break
      case "tuple":
        projected = { ...body, elements: body.elements.map(visit) }
        break
      case "map":
        projected = { ...body, key: visit(body.key), value: visit(body.value) }
        break
      case "option":
      case "list":
      case "fixed-list":
        projected = { ...body, element: visit(body.element) }
        break
      case "result":
        projected = { ...body, ok: body.ok && visit(body.ok), err: body.err && visit(body.err) }
        break
    }
    return { ...type, body: projected }
  }
  const projected = {
    root: visit(graph.root),
    defs: new Map(
      [...graph.defs].map(([id, definition]) => [
        refId(id),
        { ...definition, body: visit(definition.body) },
      ]),
    ),
  }
  const errors = validateSchemaGraph(projected)
  if (errors.length > 0) throw new Error(errors.map((error) => error.message).join("; "))
  return projected
}

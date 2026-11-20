import { type SchemaGraph, type SchemaType, type SchemaTypeBody } from '../internal/schema-model';

/** Config names are canonical; catalog selectors are values and never pass through this function. */
export function canonicalConfigName(name: string): string {
  if (name.startsWith('$')) throw new TypeError("Config field names cannot have a leading '$'");
  return name
    .replace(/[_-]+([a-zA-Z0-9])/g, (_, char: string) => char.toUpperCase())
    .replace(/^[A-Z]/, (char) => char.toLowerCase());
}

/** Keep source codecs positional while projecting their record and discriminator names. */
export function canonicalConfigGraph(source: SchemaGraph): SchemaGraph {
  const visit = (type: SchemaType): SchemaType => {
    const body = type.body;
    let projected: SchemaTypeBody = body;
    switch (body.tag) {
      case 'record': {
        const names = new Set<string>();
        projected = {
          ...body,
          fields: body.fields.map((field) => {
            const name = canonicalConfigName(field.name);
            if (names.has(name)) throw new TypeError(`Duplicate canonical config field '${name}'`);
            names.add(name);
            return { ...field, name, body: visit(field.body) };
          }),
        };
        break;
      }
      case 'union':
        projected = {
          ...body,
          branches: body.branches.map((branch) => {
            const rule = branch.discriminator;
            const discriminator =
              rule.tag === 'field-equals'
                ? {
                    ...rule,
                    val: { ...rule.val, fieldName: canonicalConfigName(rule.val.fieldName) },
                  }
                : rule.tag === 'field-absent'
                  ? { ...rule, val: canonicalConfigName(rule.val) }
                  : rule;
            return { ...branch, discriminator, body: visit(branch.body) };
          }),
        };
        break;
      case 'tuple':
        projected = { ...body, elements: body.elements.map(visit) };
        break;
      case 'variant':
        projected = {
          ...body,
          cases: body.cases.map((entry) => ({
            ...entry,
            payload: entry.payload && visit(entry.payload),
          })),
        };
        break;
      case 'map':
        projected = { ...body, key: visit(body.key), value: visit(body.value) };
        break;
      case 'option':
      case 'list':
      case 'fixed-list':
        projected = { ...body, element: visit(body.element) };
        break;
      case 'result':
        projected = { ...body, ok: body.ok && visit(body.ok), err: body.err && visit(body.err) };
        break;
      case 'secret':
      case 'quota-token':
      case 'permission-card':
      case 'stream':
      case 'future':
        throw new TypeError(`Static config cannot contain '${body.tag}' capabilities`);
    }
    return { ...type, body: projected };
  };
  return {
    root: visit(source.root),
    defs: new Map(
      [...source.defs].map(([id, definition]) => [
        id,
        { ...definition, body: visit(definition.body) },
      ]),
    ),
  };
}

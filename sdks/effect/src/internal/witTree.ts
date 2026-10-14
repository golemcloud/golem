/**
 * Compatibility-free façade over the schema-native graph/tree conversion.
 *
 * @since 1.6.0
 */
export {
  assertSchemaValueRepresentable,
  drainUnconsumedQuotaAndPermissionCardHandles,
  GraphEncoder,
  preflightWitTypedSchemaValue,
  preflightWitValueTree,
  schemaGraphFromWit,
  schemaGraphToWit,
  schemaValueFromWit,
  schemaValueToWit,
  schemaValueToWitAsync,
  typedSchemaValueFromWit,
  typedSchemaValueToWit,
} from "./schema-model/wit.js"

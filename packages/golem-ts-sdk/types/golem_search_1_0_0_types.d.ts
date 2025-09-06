/**
 * Core types and error handling for universal search interfaces
 */
declare module 'golem:search/types@1.0.0' {
  /**
   * Common structured errors for search operations
   */
  export type SearchError = {
    tag: 'index-not-found'
  } |
  {
    tag: 'invalid-query'
    val: string
  } |
  {
    tag: 'unsupported'
  } |
  {
    tag: 'internal'
    val: string
  } |
  {
    tag: 'timeout'
  } |
  {
    tag: 'rate-limited'
  };
  /**
   * Identifier types
   */
  export type IndexName = string;
  export type DocumentId = string;
  export type Json = string;
  /**
   * Document payload
   */
  export type Doc = {
    id: DocumentId;
    content: Json;
  };
  /**
   * Highlight configuration
   */
  export type HighlightConfig = {
    fields: string[];
    preTag: string | undefined;
    postTag: string | undefined;
    maxLength: number | undefined;
  };
  /**
   * Advanced search tuning
   */
  export type SearchConfig = {
    timeoutMs: number | undefined;
    boostFields: [string, number][];
    attributesToRetrieve: string[];
    language: string | undefined;
    typoTolerance: boolean | undefined;
    exactMatchBoost: number | undefined;
    providerParams: Json | undefined;
  };
  /**
   * Search request
   */
  export type SearchQuery = {
    q: string | undefined;
    filters: string[];
    sort: string[];
    facets: string[];
    page: number | undefined;
    perPage: number | undefined;
    offset: number | undefined;
    highlight: HighlightConfig | undefined;
    config: SearchConfig | undefined;
  };
  /**
   * Search hit
   */
  export type SearchHit = {
    id: DocumentId;
    score: number | undefined;
    content: Json | undefined;
    highlights: Json | undefined;
  };
  /**
   * Search result set
   */
  export type SearchResults = {
    total: number | undefined;
    page: number | undefined;
    perPage: number | undefined;
    hits: SearchHit[];
    facets: Json | undefined;
    tookMs: number | undefined;
  };
  /**
   * Field schema types
   */
  export type FieldType = "text" | "keyword" | "integer" | "float" | "boolean" | "date" | "geo-point";
  /**
   * Field definition
   */
  export type SchemaField = {
    name: string;
    fieldType: FieldType;
    required: boolean;
    facet: boolean;
    sort: boolean;
    index: boolean;
  };
  /**
   * Index schema
   */
  export type Schema = {
    fields: SchemaField[];
    primaryKey: string | undefined;
  };
}

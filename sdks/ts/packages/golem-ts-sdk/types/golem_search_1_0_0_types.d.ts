/**
 * Core types and error handling for universal search interfaces
 */
declare module 'golem:search/types@1.0.0' {
  /**
   * Common structured errors for search operations
   */
  export type SearchError = 
  {
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
    preTag?: string;
    postTag?: string;
    maxLength?: number;
  };
  /**
   * Advanced search tuning
   */
  export type SearchConfig = {
    timeoutMs?: number;
    boostFields: [string, number][];
    attributesToRetrieve: string[];
    language?: string;
    typoTolerance?: boolean;
    exactMatchBoost?: number;
    providerParams?: Json;
  };
  /**
   * Search request
   */
  export type SearchQuery = {
    q?: string;
    filters: string[];
    sort: string[];
    facets: string[];
    page?: number;
    perPage?: number;
    offset?: number;
    highlight?: HighlightConfig;
    config?: SearchConfig;
  };
  /**
   * Search hit
   */
  export type SearchHit = {
    id: DocumentId;
    score?: number;
    content?: Json;
    highlights?: Json;
  };
  /**
   * Search result set
   */
  export type SearchResults = {
    total?: number;
    page?: number;
    perPage?: number;
    hits: SearchHit[];
    facets?: Json;
    tookMs?: number;
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
    primaryKey?: string;
  };
  export type CreateIndexOptions = {
    indexName: string;
    schema?: Schema;
  };
}

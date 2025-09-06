/**
 * Unified search interface
 */
declare module 'golem:search/core@1.0.0' {
  import * as golemSearch100Types from 'golem:search/types@1.0.0';
  /**
   * Index lifecycle
   */
  export function createIndex(name: IndexName, schema: Schema | undefined): Result<void, SearchError>;
  export function deleteIndex(name: IndexName): Result<void, SearchError>;
  export function listIndexes(): Result<IndexName[], SearchError>;
  /**
   * Document operations
   */
  export function upsert(index: IndexName, doc: Doc): Result<void, SearchError>;
  export function upsertMany(index: IndexName, docs: Doc[]): Result<void, SearchError>;
  export function delete_(index: IndexName, id: DocumentId): Result<void, SearchError>;
  export function deleteMany(index: IndexName, ids: DocumentId[]): Result<void, SearchError>;
  export function get(index: IndexName, id: DocumentId): Result<Doc | undefined, SearchError>;
  /**
   * Query
   */
  export function search(index: IndexName, query: SearchQuery): Result<SearchResults, SearchError>;
  export function streamSearch(index: IndexName, query: SearchQuery): Result<SearchStream, SearchError>;
  /**
   * Schema inspection
   */
  export function getSchema(index: IndexName): Result<Schema, SearchError>;
  export function updateSchema(index: IndexName, schema: Schema): Result<void, SearchError>;
  export class SearchStream {
    getNext(): SearchHit[] | undefined;
    blockingGetNext(): SearchHit[];
  }
  export type IndexName = golemSearch100Types.IndexName;
  export type DocumentId = golemSearch100Types.DocumentId;
  export type Doc = golemSearch100Types.Doc;
  export type SearchQuery = golemSearch100Types.SearchQuery;
  export type SearchResults = golemSearch100Types.SearchResults;
  export type SearchHit = golemSearch100Types.SearchHit;
  export type Schema = golemSearch100Types.Schema;
  export type SearchError = golemSearch100Types.SearchError;
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}

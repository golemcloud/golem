/**
 * Unified search interface
 */
declare module 'golem:search/core@1.0.0' {
  import * as golemSearch100Types from 'golem:search/types@1.0.0';
  /**
   * Index lifecycle
   * @throws SearchError
   */
  export function createIndex(options: CreateIndexOptions): void;
  /**
   * @throws SearchError
   */
  export function deleteIndex(name: IndexName): void;
  /**
   * @throws SearchError
   */
  export function listIndexes(): IndexName[];
  /**
   * Document operations
   * @throws SearchError
   */
  export function upsert(index: IndexName, doc: Doc): void;
  /**
   * @throws SearchError
   */
  export function upsertMany(index: IndexName, docs: Doc[]): void;
  /**
   * @throws SearchError
   */
  export function delete_(index: IndexName, id: DocumentId): void;
  /**
   * @throws SearchError
   */
  export function deleteMany(index: IndexName, ids: DocumentId[]): void;
  /**
   * @throws SearchError
   */
  export function get(index: IndexName, id: DocumentId): Doc | undefined;
  /**
   * Query
   * @throws SearchError
   */
  export function search(index: IndexName, query: SearchQuery): SearchResults;
  /**
   * @throws SearchError
   */
  export function streamSearch(index: IndexName, query: SearchQuery): SearchStream;
  /**
   * Schema inspection
   * @throws SearchError
   */
  export function getSchema(index: IndexName): Schema;
  /**
   * @throws SearchError
   */
  export function updateSchema(index: IndexName, schema: Schema): void;
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
  export type CreateIndexOptions = golemSearch100Types.CreateIndexOptions;
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}

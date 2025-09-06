declare module 'golem:web-search/web-search@1.0.0' {
  import * as golemWebSearch100Types from 'golem:web-search/types@1.0.0';
  /**
   * Start a search session, returning a search context
   */
  export function startSearch(params: SearchParams): Result<SearchSession, SearchError>;
  /**
   * One-shot search that returns results immediately (limited result count)
   */
  export function searchOnce(params: SearchParams): Result<[SearchResult[], SearchMetadata | undefined], SearchError>;
  export class SearchSession {
    /**
     * Get the next page of results
     */
    nextPage(): Result<SearchResult[], SearchError>;
    /**
     * Retrieve session metadata (after any query)
     */
    getMetadata(): SearchMetadata | undefined;
  }
  export type SearchParams = golemWebSearch100Types.SearchParams;
  export type SearchResult = golemWebSearch100Types.SearchResult;
  export type SearchMetadata = golemWebSearch100Types.SearchMetadata;
  export type SearchError = golemWebSearch100Types.SearchError;
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}

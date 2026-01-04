declare module 'golem:web-search/web-search@1.0.0' {
  import * as golemWebSearch100Types from 'golem:web-search/types@1.0.0';
  /**
   * Start a search session, returning a search context
   * @throws SearchError
   */
  export function startSearch(params: SearchParams): SearchSession;
  /**
   * One-shot search that returns results immediately (limited result count)
   * @throws SearchError
   */
  export function searchOnce(params: SearchParams): [SearchResult[], SearchMetadata | undefined];
  export class SearchSession {
    /**
     * Get the next page of results
     * @throws SearchError
     */
    nextPage(): SearchResult[];
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

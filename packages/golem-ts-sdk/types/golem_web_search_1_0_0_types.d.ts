declare module 'golem:web-search/types@1.0.0' {
  /**
   * Optional image-related result data
   */
  export type ImageResult = {
    url: string;
    description: string | undefined;
  };
  /**
   * Core structure for a single search result
   */
  export type SearchResult = {
    title: string;
    url: string;
    snippet: string;
    displayUrl: string | undefined;
    source: string | undefined;
    score: number | undefined;
    htmlSnippet: string | undefined;
    datePublished: string | undefined;
    images: ImageResult[] | undefined;
    contentChunks: string[] | undefined;
  };
  /**
   * Safe search settings
   */
  export type SafeSearchLevel = "off" | "medium" | "high";
  /**
   * Rate limiting metadata
   */
  export type RateLimitInfo = {
    limit: number;
    remaining: number;
    resetTimestamp: bigint;
  };
  /**
   * Optional metadata for a search session
   */
  export type SearchMetadata = {
    query: string;
    totalResults: bigint | undefined;
    searchTimeMs: number | undefined;
    safeSearch: SafeSearchLevel | undefined;
    language: string | undefined;
    region: string | undefined;
    nextPageToken: string | undefined;
    rateLimits: RateLimitInfo | undefined;
    currentPage: number;
  };
  /**
   * Supported time range filtering
   */
  export type TimeRange = "day" | "week" | "month" | "year";
  /**
   * Query parameters accepted by the unified search API
   */
  export type SearchParams = {
    query: string;
    safeSearch: SafeSearchLevel | undefined;
    language: string | undefined;
    region: string | undefined;
    maxResults: number | undefined;
    timeRange: TimeRange | undefined;
    includeDomains: string[] | undefined;
    excludeDomains: string[] | undefined;
    includeImages: boolean | undefined;
    includeHtml: boolean | undefined;
    advancedAnswer: boolean | undefined;
  };
  /**
   * Structured search error
   */
  export type SearchError = {
    tag: 'invalid-query'
  } |
  {
    tag: 'rate-limited'
    val: number
  } |
  {
    tag: 'unsupported-feature'
    val: string
  } |
  {
    tag: 'backend-error'
    val: string
  };
}

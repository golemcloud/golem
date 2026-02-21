declare module 'golem:web-search/types@1.0.0' {
  /**
   * Optional image-related result data
   */
  export type ImageResult = {
    url: string;
    description?: string;
  };
  /**
   * Core structure for a single search result
   */
  export type SearchResult = {
    title: string;
    url: string;
    snippet: string;
    displayUrl?: string;
    source?: string;
    score?: number;
    htmlSnippet?: string;
    datePublished?: string;
    images?: ImageResult[];
    contentChunks?: string[];
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
    totalResults?: bigint;
    searchTimeMs?: number;
    safeSearch?: SafeSearchLevel;
    language?: string;
    region?: string;
    nextPageToken?: string;
    rateLimits?: RateLimitInfo;
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
    safeSearch?: SafeSearchLevel;
    language?: string;
    region?: string;
    maxResults?: number;
    timeRange?: TimeRange;
    includeDomains?: string[];
    excludeDomains?: string[];
    includeImages?: boolean;
    includeHtml?: boolean;
    advancedAnswer?: boolean;
  };
  /**
   * Structured search error
   */
  export type SearchError = 
  {
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

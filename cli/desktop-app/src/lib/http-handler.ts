/**
 * HTTP Handler utilities for detecting and handling HTTP incoming handler functions
 */

/**
 * Checks if an export name represents an HTTP incoming handler
 * @param exportName - The export name to check
 * @returns true if it's an HTTP incoming handler
 */
export function isHttpIncomingHandler(exportName: string): boolean {
  if (!exportName) return false;

  const normalizedName = exportName.toLowerCase();

  // Check for various HTTP incoming handler patterns
  const patterns = [
    "http/incoming-handler",
    "wasi:http/incoming-handler",
    "golem:http/incoming-handler",
  ];

  return patterns.some(pattern =>
    normalizedName.includes(pattern.toLowerCase()),
  );
}

/**
 * Checks if a function is an HTTP handler function based on export name only
 * @param exportName - The export name
 * @param functionName - The function name (ignored, kept for compatibility)
 * @returns true if it's an HTTP handler function
 */
export function isHttpHandlerFunction(exportName: string): boolean {
  const isHttpHandler = isHttpIncomingHandler(exportName);

  return isHttpHandler;
}

/**
 * Checks if HTTP handler can be directly invoked (based on export name only)
 * @param exportName - The export name
 * @param functionName - The function name (ignored, kept for compatibility)
 * @param functionDetails - The function details (ignored, not needed for HTTP handlers)
 * @returns false for HTTP handlers (they cannot be invoked directly), true for other functions
 */
export function canInvokeHttpHandler(exportName: string): boolean {
  // If it's an HTTP handler based on export name, it cannot be invoked directly
  if (isHttpHandlerFunction(exportName)) {
    return false;
  }

  return true; // Not an HTTP handler, normal invocation rules apply
}

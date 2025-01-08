import { UseMutationOptions, UseQueryOptions } from "@tanstack/react-query";

import { GolemError } from "../types/error";
import toast from "react-hot-toast";

/**
 * Displays an error message using toast notifications.
 * Handles both GolemError and standard Error types.
 *
 * @param error - The error to display (can be GolemError, Error, or unknown)
 * @param title - Optional title/context for the error
 */
export const displayError = (error: unknown, title?: string) => {
  let errorMessage = "An unexpected error occurred";

  // Handle GolemError type
  if (error && typeof error === "object" && "golemError" in error) {
    const golemError = error as GolemError;
    if (golemError.golemError) {
      errorMessage = `${golemError.golemError.type}: ${golemError.golemError.details || ""}`;
    } else if (golemError.errors?.length) {
      errorMessage = golemError.errors.join("\n");
    } else if (golemError.error) {
      errorMessage = golemError.error;
    }
  }
  // handle {"errors":["string"]}
  else if (error && typeof error === "object" && "errors" in error) {
    const golemError = error as GolemError;
    if (golemError.errors?.length) {
      errorMessage = golemError.errors.join("\n");
    }
  }
  //   handle {"error":"string"}
  else if (error && typeof error === "object" && "error" in error) {
    const golemError = error as GolemError;
    if (golemError.error) {
      errorMessage = golemError.error;
    }
  }
  // Handle standard Error type
  else if (error instanceof Error) {
    errorMessage = error.message;
  }
  // Handle string error
  else if (typeof error === "string") {
    errorMessage = error;
  }

  // Format the error message with title if provided
  const formattedMessage = title ? `${title}\n${errorMessage}` : errorMessage;

  // Show the toast with custom styling
  toast.error(formattedMessage, {
    duration: 4000, // 4 seconds
    style: {
      background: "#1F2937",
      color: "#F3F4F6",
      borderLeft: "4px solid #EF4444",
      padding: "16px",
      marginBottom: "8px",
      whiteSpace: "pre-line", // Preserve line breaks
      maxWidth: "500px",
      boxShadow:
        "0 4px 6px -1px rgba(0, 0, 0, 0.1), 0 2px 4px -1px rgba(0, 0, 0, 0.06)",
    },
    icon: "⚠️",
    // bottom right
    position: "bottom-right",
  });
};

/**
 * Creates default error handling options for React Query queries
 *
 * @param errorTitle - Title to display in the error toast
 * @returns Partial query options with error handling
 */
export const createQueryErrorConfig = <TData, TError = GolemError>(
  errorTitle?: string
): Partial<UseQueryOptions<TData, TError>> => ({
  retry: 1, // Only retry once
  retryDelay: (attemptIndex: number) =>
    Math.min(1000 * 2 ** attemptIndex, 30000),
  onError: (error: Error | GolemError) => displayError(error, errorTitle),
});

/**
 * Creates default error handling options for React Query mutations
 *
 * @param errorTitle - Title to display in the error toast
 * @returns Partial mutation options with error handling
 */
export const createMutationErrorConfig = <
  TData,
  TError = GolemError,
  TVariables = void,
  TContext = unknown,
>(
  errorTitle?: string
): Partial<UseMutationOptions<TData, TError, TVariables, TContext>> => ({
  onError: (error: Error | GolemError) => displayError(error, errorTitle),
});

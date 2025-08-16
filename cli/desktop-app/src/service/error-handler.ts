import { parseErrorMessage } from "@/lib/utils.ts";
import { toast } from "@/hooks/use-toast.ts";

export type ErrorResponse = {
  code?: string;
  title: string;
  description: string;
  payload: unknown;
};

interface GolemError {
  type: string;
  reason?: string;
  path?: string;
  worker_id?: string;
  component_id?: string;
  promise_id?: string;
  details?: string;
  expected?: string;
  got?: string;
  shard_id?: string;
  shard_ids?: string[];
}

interface ErrorResponseObject {
  error?: string;
  Error?: string;
  errors?: string[];
  golemError?: GolemError;
}

export function parseErrorResponse(response: unknown): ErrorResponse {
  const parsedError: ErrorResponse = {
    title: "API request failed.",
    description: "Something went wrong. Please try again later.",
    payload: response,
  };
  if (typeof response === "string") {
    parsedError.description = parseErrorMessage(response);
  } else if (typeof response === "object" && response !== null) {
    const errorObj = response as ErrorResponseObject;
    if (errorObj.error) {
      parsedError.description = errorObj.error;
    } else if (errorObj.Error) {
      parsedError.description = errorObj.Error;
    } else if (errorObj.errors) {
      parsedError.description = errorObj.errors.join(", ");
    } else if (errorObj.golemError) {
      parsedError.code = errorObj.golemError.type;
      parsedError.title = "Golem Error";
      parsedError.description =
        errorObj.golemError.reason || parsedError.description;

      if (errorObj.golemError?.path) {
        parsedError.description += ` (Path: ${errorObj.golemError.path})`;
      }
      switch (errorObj.golemError.type) {
        case "WorkerAlreadyExists":
          parsedError.title = "Worker Conflict";
          parsedError.description = `Worker '${errorObj.golemError.worker_id}' already exists.`;
          break;
        case "WorkerNotFound":
          parsedError.title = "Worker Not Found";
          parsedError.description = `Worker '${errorObj.golemError.worker_id}' not found.`;
          break;
        case "WorkerCreationFailed":
          parsedError.title = "Worker Creation Failed";
          parsedError.description = `Failed to create worker '${errorObj.golemError.worker_id}': ${errorObj.golemError.details}`;
          break;
        case "ComponentDownloadFailed":
          parsedError.title = "Component Download Error";
          parsedError.description = `Failed to download component '${errorObj.golemError.component_id}': ${errorObj.golemError.reason}`;
          break;
        case "ComponentParseFailed":
          parsedError.title = "Component Parsing Error";
          parsedError.description = `Failed to parse component '${errorObj.golemError.component_id}': ${errorObj.golemError.reason}`;
          break;
        case "InitialComponentFileDownloadFailed":
          parsedError.title = "File Download Failure";
          parsedError.description = `Failed to download initial file at '${errorObj.golemError.path}': ${errorObj.golemError.reason}`;
          break;
        case "PromiseNotFound":
          parsedError.title = "Promise Not Found";
          parsedError.description = `Promise '${errorObj.golemError.promise_id}' not found.`;
          break;
        case "PromiseDropped":
          parsedError.title = "Promise Dropped";
          parsedError.description = `Promise '${errorObj.golemError.promise_id}' was dropped.`;
          break;
        case "RuntimeError":
          parsedError.title = "Runtime Error";
          parsedError.description = `Runtime error occurred: ${errorObj.golemError.details || errorObj.golemError.reason || "Unknown error"}`;
          break;
        case "ValueMismatch":
          parsedError.title = "Value Mismatch";
          parsedError.description = `Value mismatch error: ${errorObj.golemError.details}`;
          break;
        case "InvalidRequest":
          parsedError.title = "Invalid Request";
          parsedError.description = `Invalid request: ${errorObj.golemError.details}`;
          break;
        case "UnexpectedOplogEntry":
          parsedError.title = "Unexpected Oplog Entry";
          parsedError.description = `Unexpected oplog entry: Expected '${errorObj.golemError.expected}', got '${errorObj.golemError.got}'.`;
          break;
        case "InvalidShardId":
          parsedError.title = "Invalid Shard ID";
          parsedError.description = `Invalid shard ID '${errorObj.golemError.shard_id}', valid IDs: ${errorObj.golemError.shard_ids?.join(", ")}.`;
          break;
        case "FileSystemError":
          parsedError.title = "File System Error";
          parsedError.description = `File system error at '${errorObj.golemError.path}': ${errorObj.golemError.reason}`;
          break;
        case "Unknown":
          parsedError.title = "Unknown Error";
          parsedError.description = `Unknown error occurred: ${errorObj.golemError.details}`;
          break;
        default:
          parsedError.title = "Golem Error";
          parsedError.description =
            errorObj.golemError.reason ||
            "An unspecified Golem error occurred.";
          break;
      }

      // Add path information after switch statement for cases that don't handle path internally
      if (
        errorObj.golemError?.path &&
        !["InitialComponentFileDownloadFailed", "FileSystemError"].includes(
          errorObj.golemError.type,
        )
      ) {
        parsedError.description += ` (Path: ${errorObj.golemError.path})`;
      }
    } else {
      // For empty objects or objects without known error properties, keep default description
      if (Object.keys(response).length === 0) {
        // Keep the default description for empty objects
      } else {
        parsedError.description = parseErrorMessage(String(response));
      }
    }
  }
  toast({
    title: parsedError.title,
    description: parsedError.description,
    variant: "destructive",
    duration: parsedError.description?.includes("Rib compilation error")
      ? Infinity
      : 5000,
  });

  // if (error.response) {
  //     const {status, data} = error.response;
  //     parsedError.code = status.toString();
  //     parsedError.payload = data;
  //
  //     if (typeof data === "object") {
  //         if (data.errorCode) {
  //             parsedError.code = data.errorCode;
  //         }
  //         if (data.message) {
  //             parsedError.title = "Error";
  //             parsedError.description = data.message;
  //         }
  //         if (data.error) {
  //             parsedError.title = data.error.title || parsedError.title;
  //             parsedError.description = data.error.detail || parsedError.description;
  //         }
  //     }
  // } else if (error.message) {
  //     parsedError.description = error.message;
  // }

  return parsedError;
}

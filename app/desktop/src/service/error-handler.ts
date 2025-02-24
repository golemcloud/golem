/* eslint-disable @typescript-eslint/no-explicit-any */
import { parseErrorMessage } from "@/lib/utils.ts";
import { toast } from "@/hooks/use-toast.ts";

export type ErrorResponse = {
  code?: string;
  title: string;
  description: string;
  payload: unknown;
};

export function parseErrorResponse(response: any): ErrorResponse {
  const parsedError: ErrorResponse = {
    title: "API request failed.",
    description: "Something went wrong. Please try again later.",
    payload: response,
  };
  if (typeof response === "string") {
    parsedError.description = parseErrorMessage(response);
  } else if (typeof response === "object") {
    if (response?.error) {
      parsedError.description = response?.error;
    } else if (response?.Error) {
      parsedError.description = response?.Error;
    } else if (response?.errors) {
      parsedError.description = response?.errors.join(", ");
    } else if (response?.golemError) {
      parsedError.code = response.golemError.type;
      parsedError.title = "Golem Error";
      parsedError.description =
        response.golemError.reason || parsedError.description;

      if (response.golemError?.path) {
        parsedError.description += ` (Path: ${response.golemError.path})`;
      }
      switch (response.golemError.type) {
        case "WorkerAlreadyExists":
          parsedError.title = "Worker Conflict";
          parsedError.description = `Worker '${response.golemError.worker_id}' already exists.`;
          break;
        case "WorkerNotFound":
          parsedError.title = "Worker Not Found";
          parsedError.description = `Worker '${response.golemError.worker_id}' not found.`;
          break;
        case "WorkerCreationFailed":
          parsedError.title = "Worker Creation Failed";
          parsedError.description = `Failed to create worker '${response.golemError.worker_id}': ${response.golemError.details}`;
          break;
        case "ComponentDownloadFailed":
          parsedError.title = "Component Download Error";
          parsedError.description = `Failed to download component '${response.golemError.component_id}': ${response.golemError.reason}`;
          break;
        case "ComponentParseFailed":
          parsedError.title = "Component Parsing Error";
          parsedError.description = `Failed to parse component '${response.golemError.component_id}': ${response.golemError.reason}`;
          break;
        case "InitialComponentFileDownloadFailed":
          parsedError.title = "File Download Failure";
          parsedError.description = `Failed to download initial file at '${response.golemError.path}': ${response.golemError.reason}`;
          break;
        case "PromiseNotFound":
          parsedError.title = "Promise Not Found";
          parsedError.description = `Promise '${response.golemError.promise_id}' not found.`;
          break;
        case "PromiseDropped":
          parsedError.title = "Promise Dropped";
          parsedError.description = `Promise '${response.golemError.promise_id}' was dropped.`;
          break;
        case "RuntimeError":
          parsedError.title = "Runtime Error";
          parsedError.description = `Runtime error occurred: ${response.golemError.details}`;
          break;
        case "ValueMismatch":
          parsedError.title = "Value Mismatch";
          parsedError.description = `Value mismatch error: ${response.golemError.details}`;
          break;
        case "InvalidRequest":
          parsedError.title = "Invalid Request";
          parsedError.description = `Invalid request: ${response.golemError.details}`;
          break;
        case "UnexpectedOplogEntry":
          parsedError.title = "Unexpected Oplog Entry";
          parsedError.description = `Unexpected oplog entry: Expected '${response.golemError.expected}', got '${response.golemError.got}'.`;
          break;
        case "InvalidShardId":
          parsedError.title = "Invalid Shard ID";
          parsedError.description = `Invalid shard ID '${response.golemError.shard_id}', valid IDs: ${response.golemError.shard_ids?.join(", ")}.`;
          break;
        case "FileSystemError":
          parsedError.title = "File System Error";
          parsedError.description = `File system error at '${response.golemError.path}': ${response.golemError.reason}`;
          break;
        case "Unknown":
          parsedError.title = "Unknown Error";
          parsedError.description = `Unknown error occurred: ${response.golemError.details}`;
          break;
        default:
          parsedError.title = "Golem Error";
          parsedError.description =
            response.golemError.reason ||
            "An unspecified Golem error occurred.";
          break;
      }
    } else {
      parsedError.description = parseErrorMessage(String(response));
    }
  }
  toast({
    title: parsedError.title,
    description: parsedError.description,
    variant: "destructive",
    duration: parsedError.description.includes("Rib compilation error")
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

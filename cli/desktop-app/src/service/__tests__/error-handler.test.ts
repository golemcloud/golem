import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { parseErrorResponse } from "../error-handler";
import { toast } from "@/hooks/use-toast";
import { parseErrorMessage } from "@/lib/utils";

// Mock dependencies
vi.mock("@/hooks/use-toast", () => ({
  toast: vi.fn(),
}));

vi.mock("@/lib/utils", () => ({
  parseErrorMessage: vi.fn(),
}));

describe("parseErrorResponse", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("should handle string error responses", () => {
    const mockParsedMessage = "Parsed error message";
    (parseErrorMessage as unknown as ReturnType<typeof vi.fn>).mockReturnValue(
      mockParsedMessage,
    );

    const result = parseErrorResponse("Raw error string");

    expect(parseErrorMessage).toHaveBeenCalledWith("Raw error string");
    expect(result).toEqual({
      title: "API request failed.",
      description: mockParsedMessage,
      payload: "Raw error string",
    });
    expect(toast).toHaveBeenCalledWith({
      title: "API request failed.",
      description: mockParsedMessage,
      variant: "destructive",
      duration: 5000,
    });
  });

  it("should handle object with error property", () => {
    const errorObj = { error: "Something went wrong" };
    const result = parseErrorResponse(errorObj);

    expect(result).toEqual({
      title: "API request failed.",
      description: "Something went wrong",
      payload: errorObj,
    });
  });

  it("should handle object with Error property", () => {
    const errorObj = { Error: "Capitalized error message" };
    const result = parseErrorResponse(errorObj);

    expect(result).toEqual({
      title: "API request failed.",
      description: "Capitalized error message",
      payload: errorObj,
    });
  });

  it("should handle object with errors array", () => {
    const errorObj = { errors: ["Error 1", "Error 2", "Error 3"] };
    const result = parseErrorResponse(errorObj);

    expect(result).toEqual({
      title: "API request failed.",
      description: "Error 1, Error 2, Error 3",
      payload: errorObj,
    });
  });

  describe("Golem specific errors", () => {
    it("should handle WorkerAlreadyExists error", () => {
      const errorObj = {
        golemError: {
          type: "WorkerAlreadyExists",
          worker_id: "test-worker-123",
          reason: "Worker already exists",
        },
      };

      const result = parseErrorResponse(errorObj);

      expect(result).toEqual({
        code: "WorkerAlreadyExists",
        title: "Worker Conflict",
        description: "Worker 'test-worker-123' already exists.",
        payload: errorObj,
      });
    });

    it("should handle WorkerNotFound error", () => {
      const errorObj = {
        golemError: {
          type: "WorkerNotFound",
          worker_id: "missing-worker-456",
          reason: "Worker not found",
        },
      };

      const result = parseErrorResponse(errorObj);

      expect(result).toEqual({
        code: "WorkerNotFound",
        title: "Worker Not Found",
        description: "Worker 'missing-worker-456' not found.",
        payload: errorObj,
      });
    });

    it("should handle WorkerCreationFailed error", () => {
      const errorObj = {
        golemError: {
          type: "WorkerCreationFailed",
          worker_id: "failed-worker-789",
          details: "Insufficient resources",
        },
      };

      const result = parseErrorResponse(errorObj);

      expect(result).toEqual({
        code: "WorkerCreationFailed",
        title: "Worker Creation Failed",
        description:
          "Failed to create worker 'failed-worker-789': Insufficient resources",
        payload: errorObj,
      });
    });

    it("should handle ComponentDownloadFailed error", () => {
      const errorObj = {
        golemError: {
          type: "ComponentDownloadFailed",
          component_id: "comp-123",
          reason: "Network timeout",
        },
      };

      const result = parseErrorResponse(errorObj);

      expect(result).toEqual({
        code: "ComponentDownloadFailed",
        title: "Component Download Error",
        description: "Failed to download component 'comp-123': Network timeout",
        payload: errorObj,
      });
    });

    it("should handle ComponentParseFailed error", () => {
      const errorObj = {
        golemError: {
          type: "ComponentParseFailed",
          component_id: "comp-456",
          reason: "Invalid WASM format",
        },
      };

      const result = parseErrorResponse(errorObj);

      expect(result).toEqual({
        code: "ComponentParseFailed",
        title: "Component Parsing Error",
        description:
          "Failed to parse component 'comp-456': Invalid WASM format",
        payload: errorObj,
      });
    });

    it("should handle InitialComponentFileDownloadFailed error", () => {
      const errorObj = {
        golemError: {
          type: "InitialComponentFileDownloadFailed",
          path: "/path/to/file.wasm",
          reason: "File not found",
        },
      };

      const result = parseErrorResponse(errorObj);

      expect(result).toEqual({
        code: "InitialComponentFileDownloadFailed",
        title: "File Download Failure",
        description:
          "Failed to download initial file at '/path/to/file.wasm': File not found",
        payload: errorObj,
      });
    });

    it("should handle RuntimeError error", () => {
      const errorObj = {
        golemError: {
          type: "RuntimeError",
          details: "Stack overflow occurred",
        },
      };

      const result = parseErrorResponse(errorObj);

      expect(result).toEqual({
        code: "RuntimeError",
        title: "Runtime Error",
        description: "Runtime error occurred: Stack overflow occurred",
        payload: errorObj,
      });
    });

    it("should handle FileSystemError error", () => {
      const errorObj = {
        golemError: {
          type: "FileSystemError",
          path: "/tmp/golem",
          reason: "Permission denied",
        },
      };

      const result = parseErrorResponse(errorObj);

      expect(result).toEqual({
        code: "FileSystemError",
        title: "File System Error",
        description: "File system error at '/tmp/golem': Permission denied",
        payload: errorObj,
      });
    });

    it("should handle UnexpectedOplogEntry error", () => {
      const errorObj = {
        golemError: {
          type: "UnexpectedOplogEntry",
          expected: "CreateWorker",
          got: "DeleteWorker",
        },
      };

      const result = parseErrorResponse(errorObj);

      expect(result).toEqual({
        code: "UnexpectedOplogEntry",
        title: "Unexpected Oplog Entry",
        description:
          "Unexpected oplog entry: Expected 'CreateWorker', got 'DeleteWorker'.",
        payload: errorObj,
      });
    });

    it("should handle InvalidShardId error", () => {
      const errorObj = {
        golemError: {
          type: "InvalidShardId",
          shard_id: "shard-999",
          shard_ids: ["shard-1", "shard-2", "shard-3"],
        },
      };

      const result = parseErrorResponse(errorObj);

      expect(result).toEqual({
        code: "InvalidShardId",
        title: "Invalid Shard ID",
        description:
          "Invalid shard ID 'shard-999', valid IDs: shard-1, shard-2, shard-3.",
        payload: errorObj,
      });
    });

    it("should handle Unknown golem error", () => {
      const errorObj = {
        golemError: {
          type: "Unknown",
          details: "Something unexpected happened",
        },
      };

      const result = parseErrorResponse(errorObj);

      expect(result).toEqual({
        code: "Unknown",
        title: "Unknown Error",
        description: "Unknown error occurred: Something unexpected happened",
        payload: errorObj,
      });
    });

    it("should handle unrecognized golem error type", () => {
      const errorObj = {
        golemError: {
          type: "UnrecognizedError",
          reason: "Some custom reason",
        },
      };

      const result = parseErrorResponse(errorObj);

      expect(result).toEqual({
        code: "UnrecognizedError",
        title: "Golem Error",
        description: "Some custom reason",
        payload: errorObj,
      });
    });

    it("should handle golem error with path", () => {
      const errorObj = {
        golemError: {
          type: "RuntimeError",
          reason: "Execution failed",
          path: "/api/v1/workers",
        },
      };

      const result = parseErrorResponse(errorObj);

      expect(result.description).toBe(
        "Runtime error occurred: Execution failed (Path: /api/v1/workers)",
      );
    });

    it("should handle golem error without reason", () => {
      const errorObj = {
        golemError: {
          type: "UnknownType",
        },
      };

      const result = parseErrorResponse(errorObj);

      expect(result).toEqual({
        code: "UnknownType",
        title: "Golem Error",
        description: "An unspecified Golem error occurred.",
        payload: errorObj,
      });
    });
  });

  it("should handle generic object errors", () => {
    const mockParsedMessage = "Parsed object error";
    (parseErrorMessage as unknown as ReturnType<typeof vi.fn>).mockReturnValue(
      mockParsedMessage,
    );

    const errorObj = { someProperty: "value" };
    const result = parseErrorResponse(errorObj);

    expect(parseErrorMessage).toHaveBeenCalledWith("[object Object]");
    expect(result).toEqual({
      title: "API request failed.",
      description: mockParsedMessage,
      payload: errorObj,
    });
  });

  it("should show toast with infinite duration for RIB compilation errors", () => {
    const errorObj = { error: "Rib compilation error occurred" };

    parseErrorResponse(errorObj);

    expect(toast).toHaveBeenCalledWith({
      title: "API request failed.",
      description: "Rib compilation error occurred",
      variant: "destructive",
      duration: Infinity,
    });
  });

  it("should show toast with 5000ms duration for regular errors", () => {
    const errorObj = { error: "Regular error occurred" };

    parseErrorResponse(errorObj);

    expect(toast).toHaveBeenCalledWith({
      title: "API request failed.",
      description: "Regular error occurred",
      variant: "destructive",
      duration: 5000,
    });
  });

  it("should return default error for null/undefined input", () => {
    const result = parseErrorResponse(null);

    expect(result).toEqual({
      title: "API request failed.",
      description: "Something went wrong. Please try again later.",
      payload: null,
    });
  });

  it("should return default error for empty object", () => {
    const result = parseErrorResponse({});

    expect(result).toEqual({
      title: "API request failed.",
      description: "Something went wrong. Please try again later.",
      payload: {},
    });
  });
});

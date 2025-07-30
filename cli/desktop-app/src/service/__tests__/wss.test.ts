import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { WSS } from "../wss";

// Mock dependencies
vi.mock("@/lib/tauri&web.ts", () => ({
  fetchCurrentIP: vi.fn(),
  UniversalWebSocket: {
    connect: vi.fn(),
  },
}));

interface MockWebSocket {
  send: ReturnType<typeof vi.fn>;
  close: ReturnType<typeof vi.fn>;
  onMessage: ReturnType<typeof vi.fn>;
}

describe("WSS (WebSocket Service)", () => {
  let mockWebSocket: MockWebSocket;

  beforeEach(() => {
    vi.clearAllMocks();

    mockWebSocket = {
      send: vi.fn(),
      close: vi.fn(),
      onMessage: vi.fn(),
    };
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  describe("getConnection", () => {
    it("should create WSS connection with localhost URL", async () => {
      const { UniversalWebSocket } = await import("@/lib/tauri&web.ts");
      (
        UniversalWebSocket.connect as ReturnType<typeof vi.fn>
      ).mockResolvedValue(mockWebSocket);

      const wss = await WSS.getConnection("/api/websocket");

      expect(UniversalWebSocket.connect).toHaveBeenCalledWith(
        "ws://localhost:9881/api/websocket",
      );
      expect(wss).toBeInstanceOf(WSS);
    });

    it("should replace http with ws in URL", async () => {
      const { UniversalWebSocket } = await import("@/lib/tauri&web.ts");
      (
        UniversalWebSocket.connect as ReturnType<typeof vi.fn>
      ).mockResolvedValue(mockWebSocket);

      await WSS.getConnection("/test");

      expect(UniversalWebSocket.connect).toHaveBeenCalledWith(
        "ws://localhost:9881/test",
      );
    });

    it("should handle connection failures", async () => {
      const { UniversalWebSocket } = await import("@/lib/tauri&web.ts");
      (
        UniversalWebSocket.connect as ReturnType<typeof vi.fn>
      ).mockRejectedValue(new Error("Connection failed"));

      await expect(WSS.getConnection("/fail")).rejects.toThrow(
        "Connection failed",
      );
    });

    it("should use fallback when fetchCurrentIP returns null", async () => {
      const { UniversalWebSocket } = await import("@/lib/tauri&web.ts");
      // (fetchCurrentIP as any).mockResolvedValue(null);
      (
        UniversalWebSocket.connect as ReturnType<typeof vi.fn>
      ).mockResolvedValue(mockWebSocket);

      await WSS.getConnection("/fallback");

      // Should use the hardcoded localhost fallback
      expect(UniversalWebSocket.connect).toHaveBeenCalledWith(
        "ws://localhost:9881/fallback",
      );
    });
  });

  describe("WebSocket operations", () => {
    let wss: WSS;

    beforeEach(async () => {
      const { UniversalWebSocket } = await import("@/lib/tauri&web.ts");
      (
        UniversalWebSocket.connect as ReturnType<typeof vi.fn>
      ).mockResolvedValue(mockWebSocket);
      wss = await WSS.getConnection("/test");
    });

    describe("send", () => {
      it("should send data through websocket", () => {
        const testData = { message: "test" } as never;

        wss.send(testData);

        expect(mockWebSocket.send).toHaveBeenCalledWith(testData);
      });

      it("should handle multiple send calls", () => {
        const data1 = { message: "first" } as never;
        const data2 = { message: "second" } as never;

        wss.send(data1);
        wss.send(data2);

        expect(mockWebSocket.send).toHaveBeenCalledTimes(2);
        expect(mockWebSocket.send).toHaveBeenNthCalledWith(1, data1);
        expect(mockWebSocket.send).toHaveBeenNthCalledWith(2, data2);
      });
    });

    describe("close", () => {
      it("should close websocket connection", () => {
        wss.close();

        expect(mockWebSocket.close).toHaveBeenCalled();
      });

      it("should handle multiple close calls", () => {
        wss.close();
        wss.close();

        expect(mockWebSocket.close).toHaveBeenCalledTimes(2);
      });
    });

    describe("onMessage", () => {
      it("should register message callback", () => {
        const messageHandler = vi.fn();

        wss.onMessage(messageHandler);

        expect(mockWebSocket.onMessage).toHaveBeenCalledWith(messageHandler);
      });

      it("should handle multiple message handlers", () => {
        const handler1 = vi.fn();
        const handler2 = vi.fn();

        wss.onMessage(handler1);
        wss.onMessage(handler2);

        expect(mockWebSocket.onMessage).toHaveBeenCalledTimes(2);
        expect(mockWebSocket.onMessage).toHaveBeenNthCalledWith(1, handler1);
        expect(mockWebSocket.onMessage).toHaveBeenNthCalledWith(2, handler2);
      });

      it("should pass through callback parameters correctly", () => {
        const messageHandler = vi.fn();

        wss.onMessage(messageHandler);

        // Verify the callback function signature is preserved
        expect(mockWebSocket.onMessage).toHaveBeenCalledWith(
          expect.any(Function),
        );

        // Simulate message received
        const receivedCallback = mockWebSocket.onMessage.mock.calls[0]?.[0];
        const testMessage = { type: "test", data: "message" };
        if (receivedCallback) {
          receivedCallback(testMessage);
        }

        expect(messageHandler).toHaveBeenCalledWith(testMessage);
      });
    });
  });

  describe("URL construction", () => {
    it("should handle paths with leading slash", async () => {
      const { UniversalWebSocket } = await import("@/lib/tauri&web.ts");
      (
        UniversalWebSocket.connect as ReturnType<typeof vi.fn>
      ).mockResolvedValue(mockWebSocket);

      await WSS.getConnection("/api/stream");

      expect(UniversalWebSocket.connect).toHaveBeenCalledWith(
        "ws://localhost:9881/api/stream",
      );
    });

    it("should handle paths without leading slash", async () => {
      const { UniversalWebSocket } = await import("@/lib/tauri&web.ts");
      (
        UniversalWebSocket.connect as ReturnType<typeof vi.fn>
      ).mockResolvedValue(mockWebSocket);

      await WSS.getConnection("api/stream");

      expect(UniversalWebSocket.connect).toHaveBeenCalledWith(
        "ws://localhost:9881api/stream",
      );
    });

    it("should handle empty path", async () => {
      const { UniversalWebSocket } = await import("@/lib/tauri&web.ts");
      (
        UniversalWebSocket.connect as ReturnType<typeof vi.fn>
      ).mockResolvedValue(mockWebSocket);

      await WSS.getConnection("");

      expect(UniversalWebSocket.connect).toHaveBeenCalledWith(
        "ws://localhost:9881",
      );
    });

    it("should handle complex URLs with query parameters", async () => {
      const { UniversalWebSocket } = await import("@/lib/tauri&web.ts");
      (
        UniversalWebSocket.connect as ReturnType<typeof vi.fn>
      ).mockResolvedValue(mockWebSocket);

      await WSS.getConnection("/api/stream?token=abc123&channel=main");

      expect(UniversalWebSocket.connect).toHaveBeenCalledWith(
        "ws://localhost:9881/api/stream?token=abc123&channel=main",
      );
    });
  });

  describe("Error handling", () => {
    it("should propagate connection errors", async () => {
      const { UniversalWebSocket } = await import("@/lib/tauri&web.ts");
      const connectionError = new Error("WebSocket connection failed");
      (
        UniversalWebSocket.connect as ReturnType<typeof vi.fn>
      ).mockRejectedValue(connectionError);

      await expect(WSS.getConnection("/error")).rejects.toThrow(
        "WebSocket connection failed",
      );
    });

    it("should handle errors in send operation", async () => {
      const { UniversalWebSocket } = await import("@/lib/tauri&web.ts");
      mockWebSocket.send.mockImplementation(() => {
        throw new Error("Send failed");
      });
      (
        UniversalWebSocket.connect as ReturnType<typeof vi.fn>
      ).mockResolvedValue(mockWebSocket);

      const wss = await WSS.getConnection("/test");

      expect(() => {
        wss.send({ data: "test" } as never);
      }).toThrow("Send failed");
    });

    it("should handle errors in close operation", async () => {
      const { UniversalWebSocket } = await import("@/lib/tauri&web.ts");
      mockWebSocket.close.mockImplementation(() => {
        throw new Error("Close failed");
      });
      (
        UniversalWebSocket.connect as ReturnType<typeof vi.fn>
      ).mockResolvedValue(mockWebSocket);

      const wss = await WSS.getConnection("/test");

      expect(() => {
        wss.close();
      }).toThrow("Close failed");
    });
  });

  describe("Integration scenarios", () => {
    it("should support full websocket lifecycle", async () => {
      const { UniversalWebSocket } = await import("@/lib/tauri&web.ts");
      (
        UniversalWebSocket.connect as ReturnType<typeof vi.fn>
      ).mockResolvedValue(mockWebSocket);

      // Create connection
      const wss = await WSS.getConnection("/lifecycle");

      // Setup message handler
      const messageHandler = vi.fn();
      wss.onMessage(messageHandler);

      // Send message
      const testMessage = { action: "ping" } as never;
      wss.send(testMessage);

      // Simulate receiving message
      const registeredCallback = mockWebSocket.onMessage.mock.calls[0]?.[0];
      if (registeredCallback) {
        registeredCallback({ action: "pong" });
      }

      // Close connection
      wss.close();

      // Verify all operations
      expect(UniversalWebSocket.connect).toHaveBeenCalledWith(
        "ws://localhost:9881/lifecycle",
      );
      expect(mockWebSocket.onMessage).toHaveBeenCalledWith(messageHandler);
      expect(mockWebSocket.send).toHaveBeenCalledWith(testMessage);
      expect(messageHandler).toHaveBeenCalledWith({ action: "pong" });
      expect(mockWebSocket.close).toHaveBeenCalled();
    });

    it("should handle rapid send operations", async () => {
      const { UniversalWebSocket } = await import("@/lib/tauri&web.ts");
      (
        UniversalWebSocket.connect as ReturnType<typeof vi.fn>
      ).mockResolvedValue(mockWebSocket);

      const wss = await WSS.getConnection("/rapid");

      // Send multiple messages rapidly
      const messages = Array.from({ length: 100 }, (_, i) => ({
        id: i,
        data: `message-${i}`,
      }));
      messages.forEach(msg => wss.send(msg as never));

      expect(mockWebSocket.send).toHaveBeenCalledTimes(100);
    });
  });
});

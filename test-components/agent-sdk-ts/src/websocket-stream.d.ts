interface WebSocketStreamOpenInfo {
    readable: ReadableStream<string | ArrayBuffer>;
    writable: WritableStream<string | ArrayBuffer | ArrayBufferView>;
    protocol: string;
    extensions: string;
}

interface WebSocketStreamCloseInfo {
    closeCode?: number;
    reason?: string;
}

declare class WebSocketStream {
    constructor(url: string, options?: { protocols?: string[] });
    readonly url: string;
    readonly opened: Promise<WebSocketStreamOpenInfo>;
    readonly closed: Promise<WebSocketStreamCloseInfo>;
    close(options?: WebSocketStreamCloseInfo): void;
}

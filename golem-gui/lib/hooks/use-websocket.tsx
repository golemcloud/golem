import { useEffect, useState } from 'react';
import useSWR from 'swr';

export type WebSocketMessage = {
  type: string;
  data: any; // You can replace `any` with a more specific type if known
};

const websocketFetcher = (url: string) => {
  return new Promise<WebSocket>((resolve, reject) => {
    const ws = new WebSocket(url);

    ws.onopen = () => resolve(ws);
    ws.onerror = (err) => reject(err);
  });
};

export const useWebSocket = <T extends WebSocketMessage>(url: string) => {
  const { data: socket, error } = useSWR<WebSocket>(url, websocketFetcher, {
    suspense: false,
    revalidateOnFocus: false,
    revalidateOnReconnect: false,
  });

  const [messages, setMessages] = useState<T[]>([]);
  const [isConnected, setIsConnected] = useState(false);

  useEffect(() => {
    if (!socket) return;

    const handleOpen = () => {
      setIsConnected(true);
      console.log('WebSocket connection established');
    };

    const handleMessage = (event: MessageEvent) => {
      setMessages((prev)=>[JSON.parse(event.data),...prev])
    };

    const handleClose = () => {
      setIsConnected(false);
      console.log('WebSocket connection closed');
    };

    socket.onopen = handleOpen;
    socket.onmessage = handleMessage;
    socket.onclose = handleClose;

    return () => {
      socket.close();
    };
  }, [socket]);

  const sendMessage = (message: T) => {
    if (socket?.readyState === WebSocket.OPEN) {
      socket.send(JSON.stringify(message));
    } else {
      console.error('WebSocket is not open');
    }
  };

  return {
    messages,
    sendMessage,
    isConnected,
    error,
  };
};

export const useWebSocketWithPath = <T extends WebSocketMessage>(path: string) => {
  const baseUrl = 'ws://localhost:9881';
  const fullUrl = `${baseUrl}/${path}`;

  const { messages, sendMessage, isConnected, error } = useWebSocket<T>(fullUrl);

  return { messages, sendMessage, isConnected, error };
};

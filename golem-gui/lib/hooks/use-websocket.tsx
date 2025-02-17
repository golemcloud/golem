import { GolemError } from '@/types/api';
import { EventMessage } from '@/types/api';
import { useEffect, useState } from 'react';
import useSWR from 'swr';
import { getErrorMessage } from '../utils';
import WebSocket, { Message } from '@tauri-apps/plugin-websocket';

type TauriWebSocket = Awaited<ReturnType<typeof WebSocket.connect>>;

const websocketFetcher = async (url: string) => {
  try {
    const ws = await WebSocket.connect(url);
    return ws;
  } catch (err) {
    console.error("Error connecting to WebSocket:", err);
    throw new Error(getErrorMessage(err as GolemError | string));
  }
};

export const useWebSocket = (url: string) => {
  const { data: socket, error } = useSWR<TauriWebSocket>(url, websocketFetcher, {
    suspense: false,
    revalidateOnFocus: false,
    revalidateOnReconnect: false,
    shouldRetryOnError: false,
  });

  const [messages, setMessages] = useState<EventMessage[]>([]);
  const [isConnected, setIsConnected] = useState(false);

  useEffect(() => {
    if (!socket) return;

    const handleMessage = (msg: Message) => {
      if (typeof msg.data === 'string') {
        try {
          const parsedData = JSON.parse(msg.data) as EventMessage;
          setMessages((prev) => [parsedData, ...prev].slice(0, 60));
        } catch (err) {
          console.error('Error parsing WebSocket message:', err);
        }
      } else {
        console.warn('Received non-text message:', msg);
      }
    };

    try {
      socket.addListener(handleMessage);
      setIsConnected(true);
      console.log('WebSocket connection established');
    } catch (err) {
      console.error('Error adding WebSocket listener:', err);
      setIsConnected(false);
    }

    return () => {
      try {
        socket.disconnect();
        console.log('WebSocket disconnected');
      } catch (err) {
        console.error('Error during WebSocket cleanup:', err);
      }
    };
  }, [socket]);

  const sendMessage = async (message: EventMessage) => {
    if (socket && isConnected) {
      try {
        const payload = JSON.stringify(message);
        if (typeof payload === 'string') {
          await socket.send(payload);
        } else {
          console.error('Invalid message payload:', payload);
        }
      } catch (err) {
        console.error('Error sending WebSocket message:', err);
        setIsConnected(false);
        throw err;
      }
    } else {
      console.error('WebSocket is not connected');
      throw new Error('WebSocket is not connected');
    }
  };

  return {
    messages,
    sendMessage,
    isConnected,
    error: error ? getErrorMessage(error) : undefined,
  };
};

function getBaseUrl() {
  const protocol = 'ws://';
  const host = process.env.NEXT_PUBLIC_BACKEND_API_URL?.replace(/^https?:\/\//, '').replace(/\/$/, '') 
    || 'localhost:9881';
  return process.env.NEXT_PUBLIC_WEB_SOCKET_URL?.replace(/\/$/, '') || `${protocol}${host}`;
}

export const useWebSocketWithPath = (path: string) => {
  const baseUrl = getBaseUrl();
  const cleanPath = path.replace(/^\//, '');
  const fullUrl = `${baseUrl}/${cleanPath}`;

  const { messages, sendMessage, isConnected, error } = useWebSocket(fullUrl);

  return { 
    messages, 
    sendMessage, 
    isConnected, 
    error 
  };
};
import { EventMessage } from '@/types/api';
import { useEffect, useState } from 'react';
import useSWR from 'swr';
import { getErrorMessage } from '../utils';



const websocketFetcher = (url: string) => {
  return new Promise<WebSocket>((resolve, reject) => {
    const ws = new WebSocket(url);

    ws.onopen = () => resolve(ws);
    ws.onerror = (err) =>{ 
      console.log("error occurred while connection to backend", err)
      return reject("something went wrong!");}
  });
};

export const useWebSocket = (url: string) => {
  const { data: socket, error } = useSWR<WebSocket>(url, websocketFetcher, {
    suspense: false,
    revalidateOnFocus: false,
    revalidateOnReconnect: false,
  });

  const [messages, setMessages] = useState<EventMessage[]>([]);
  const [isConnected, setIsConnected] = useState(false);

  useEffect(() => {
    if (!socket) return;

    const handleOpen = () => {
      setIsConnected(true);
      console.log('WebSocket connection established');
    };

    const handleMessage = (event: MessageEvent) => {
      setMessages((prev)=>[JSON.parse(event.data),...prev].slice(0, 60))
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

  const sendMessage = (message: EventMessage) => {
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

function getBaseUrl() {
  if (typeof window === 'undefined') {
    return '';
  }
  const isSecure = window.location.protocol === 'https:';
  //For demonstration purposes, if we use ngrok or port forwarding, the protocol might be set to https. Therefore, we use NEXT_PUBLIC_IS_LOCAL to implicitly treat it as a local environment.
  const protocol = process.env.NEXT_PUBLIC_IS_LOCAL!== "true" && isSecure ? 'wss://' : 'ws://';
  const host = process.env.NEXT_PUBLIC_BACKEND_API_URL?.replace(/^https?:\/\//, '');
  return `${protocol}${host}`;
}

export const useWebSocketWithPath = (path: string) => {
  const baseUrl = getBaseUrl();
  const fullUrl = `${baseUrl}/${path}`;

  const { messages, sendMessage, isConnected, error } = useWebSocket(fullUrl);

  return { messages, sendMessage, isConnected, error: getErrorMessage(error) };
};

"use client";

import React, {useMemo, useState } from "react";
import { Box, Divider, Tab, Tabs, Toolbar, Typography } from "@mui/material";
import { useWorker } from "@/lib/hooks/use-worker";
import { useParams } from "next/navigation";
import {
  CheckCircleOutline,
  ErrorOutline,
  RocketLaunch,
} from "@mui/icons-material";
import InvokePage from "./invoke";
import Overview from "./overview";
import TerminalPage from "./live";
import FileComponent from "./files";
import Manage from "./manage";
import { useWebSocketWithPath, WebSocketMessage } from "@/lib/hooks/use-websocket";

interface CustomMessage extends WebSocketMessage {
  type: 'custom';
  payload: { user: string; message: string };
}
const WorkerListWithDropdowns = () => {
  const [activeTab, setActiveTab] = useState(0);

  //TO DO: let show filters in url so that user can share the url to others.
  const { compId } = useParams<{ compId: string }>();
  const { id: workerName } = useParams<{ id: string }>();
  //need to integrate the filter logic here. and pagination or scroll on load needs to implemented or addd show more at the end on click we need to next set of data
  const { worker, isLoading } = useWorker(compId, workerName);


  const { messages, isConnected } = useWebSocketWithPath<CustomMessage>(`v1/components/${compId}/workers/${workerName}/connect`);

  console.log("messages", messages);
  console.log("isConnected", isConnected);

  

  // console.log("invokeMessages====>", invokeMessages);
  const workerStats = useMemo(() => {
    return [
      {
        label: "Status",
        value: worker?.status,
        icon: <CheckCircleOutline fontSize="large" />,
        isLoading: isLoading,
      },
      {
        label: "Memory Usage",
        value: `${worker?.totalLinearMemorySize}`,
        icon: <RocketLaunch fontSize="large" />,
        isLoading: isLoading,
      },
      {
        label: "Resource Count",
        value: `${worker?.ownedResources?.length ?? 0}`,
        icon: <ErrorOutline fontSize="large" />,
        isLoading: isLoading,
      },
    ];
  }, [worker]);

  const handleTabChange = (event: React.SyntheticEvent, newValue: number) => {
    setActiveTab(newValue);
  };

  return (
    <Box
      className="text-black dark:text-white"
      sx={{ width: "100%", height: "100vh" }}
    >
      <Tabs
        value={activeTab}
        onChange={handleTabChange}
        aria-label="Worker Settings Tabs"
        textColor="inherit"
      >
        <Tab label="Overview" />
        <Tab label="Live" />
        <Tab label="Environment" />
        <Tab label="Files" />
        <Tab label="Manage" />
        <Tab label="Invoke" />
      </Tabs>
      <Divider
        className="border-gray-300 dark:border-gray-700"
        sx={{ marginTop: "4px" }}
      />
      <Divider className="border-gray-300 dark:border-gray-700" sx={{ marginTop: '4px' }} />
      <Toolbar
        sx={{ justifyContent: "center", borderBottom: "1px solid #333" }}
        className="dark:border-gray-600"
      >
        <Typography
          variant="h6"
          sx={{ fontWeight: "bold" }}
          className="text-gray-700 dark:text-gray-300"
        >
          {workerName}
        </Typography>
      </Toolbar>
      {activeTab === 0 && <Overview worker={worker} isLoading={isLoading} />}
      {activeTab === 1 && <TerminalPage messages={messages}/>}
      {activeTab === 3 && <FileComponent />}
      {activeTab === 4 && <Manage />}
      {activeTab === 5 && <InvokePage />}
    </Box>
  );
};

export default WorkerListWithDropdowns;

"use client";

import React, { useState } from "react";
import { Box, Tab, Tabs } from "@mui/material";
import { useWorker } from "@/lib/hooks/use-worker";
import { useParams } from "next/navigation";
import InvokePage from "./invoke";
import Overview from "./overview";
import TerminalPage from "./live";
import FileComponent from "./files";
import Manage from "./manage";

import { useWebSocketWithPath } from "@/lib/hooks/use-websocket";
import SecondaryHeader from "@/components/ui/secondary-header";

// interface CustomMessage extends WebSocketMessage {
//   type: 'custom';
//   payload: { user: string; message: string };
// }
const WorkerListWithDropdowns = () => {
  const [activeTab, setActiveTab] = useState(0);

  //TO DO: let show filters in url so that user can share the url to others.
  const { compId } = useParams<{ compId: string }>();
  const { id: workerName } = useParams<{ id: string }>();
  //  need to integrate the filter logic here. and pagination or scroll on load needs to implemented or addd show more at the end on click we need to next set of data
  const { worker, isLoading } = useWorker(compId, workerName);

  const { messages, isConnected } = useWebSocketWithPath(
    `v1/components/${compId}/workers/${workerName}/connect`
  );

  console.log("messages", messages);
  console.log("isConnected", isConnected);

  const handleTabChange = (event: React.SyntheticEvent, newValue: number) => {
    setActiveTab(newValue);
  };

  return (
    <Box className="text-black dark:text-white">
      <SecondaryHeader variant="component" onClick={() => {}} />
      <Tabs
        value={activeTab}
        variant="scrollable"
        onChange={handleTabChange}
        className="dark:bg-[#0a0a0a8e]"
        aria-label="Worker Settings Tabs"
        textColor="inherit"
        sx={{
          paddingBottom: "5px",
          paddingLeft: "12px",
          "& .MuiTab-root": {
            textTransform: "none",
            minWidth: "80px",
            padding: "2px 2px",
          },
          "& .MuiTabs-scroller": {
            overflowX: "auto",
          },
          "@media (max-width: 600px)": {
            "& .MuiTab-root": {
              fontSize: "11px",
              minWidth: "40px",
            },
            "& .MuiTabs-flexContainer": {
              gap: "4px",
            },
          },
          "& .MuiTabs-indicator": {
            bgcolor:"#373737",
          },
        }}
      >
        <Tab label="Overview" />
        <Tab label="Live" />
        <Tab label="Environment" />
        <Tab label="Files" />
        <Tab label="Manage" />
        <Tab label="Invoke" />
      </Tabs>
      {activeTab === 0 && <Overview worker={worker} isLoading={isLoading} />}
      {activeTab === 1 && <TerminalPage messages={messages} />}
      {activeTab === 3 && <FileComponent />}
      {activeTab === 4 && <Manage />}
      {activeTab === 5 && <InvokePage worker={worker} />}
    </Box>
  );
};

export default WorkerListWithDropdowns;

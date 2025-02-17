import React, { useState } from "react";
import { Box, Tab, Tabs } from "@mui/material";
import { useWorker } from "@lib/hooks/use-worker";
import InvokePage from "@components/components/invoke"; 
import Overview from "@components/workers/overview"; 
import TerminalPage from "@components/workers/live"; 
import FileComponent from "@components/workers/files";
import Manage from "@components/workers/manage"; 

import { useWebSocketWithPath } from "@lib/hooks/use-websocket";
import SecondaryHeader from "@ui/secondary-header";
import ErrorBoundary from "@ui/error-boundary";
import EnvironmentTab from "@components/workers/environment-tab"; 
import { useCustomParam } from "@lib/hooks/use-custom-param";
import { Loader } from "lucide-react";
import ErrorPage from "@components/workers/error-page";

// interface CustomMessage extends WebSocketMessage {
//   type: 'custom';
//   payload: { user: string; message: string };
// }
const WorkerListWithDropdowns = () => {
  const [activeTab, setActiveTab] = useState(0);

  //TO DO: let show filters in url so that user can share the url to others.
  const { compId } = useCustomParam();
  const { id: workerName } = useCustomParam();
  //  need to integrate the filter logic here. and pagination or scroll on load needs to implemented or addd show more at the end on click we need to next set of data
  const { worker, isLoading, error } = useWorker(compId, workerName);

  const { messages, isConnected, error: websocketError } = useWebSocketWithPath(
    `v1/components/${compId}/workers/${workerName}/connect`
  );

  console.log("messages", messages);
  console.log("isConnected", isConnected);

  const handleTabChange = (event: React.SyntheticEvent, newValue: number) => {
    setActiveTab(newValue);
  };

  const isFailed = worker && worker.status === "Failed" 


  //if worker status is failed. then we don't need to show the webscoket error.
  if (error || (websocketError && (!isLoading && !isFailed))) {
      return <ErrorBoundary message={error|| websocketError}/>
  }


  if(isLoading){
    return <Loader/>
  }

  return (
    <Box className="text-black dark:text-white">
      <SecondaryHeader variant="component" onClick={() => {}} />
      {!isFailed && <Tabs
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
      </Tabs>}
      {isFailed && <ErrorPage worker={worker}/>}
      {!isFailed && activeTab === 0 && <Overview worker={worker} isLoading={isLoading} messages={messages}/>}
      {!isFailed && activeTab === 2 && <EnvironmentTab worker={worker} />}
      {!isFailed && activeTab === 1 && <TerminalPage messages={messages} />}
      {!isFailed && activeTab === 3 && <FileComponent />}
      {!isFailed && activeTab === 4 && <Manage />}
      {!isFailed && activeTab === 5 && <InvokePage worker={worker} />}
    </Box>
  );
};

export default WorkerListWithDropdowns;

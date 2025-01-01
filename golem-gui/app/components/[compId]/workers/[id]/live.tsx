import React, { useState } from "react";
import {
  Box,
  AppBar,
  Toolbar,
  Tabs,
  Tab,
  Button,
  Divider,
} from "@mui/material";
import Logs from "./logs";
import InvocationLogs from "./invoke-logs";
import TerminalLogs from "./terminal";
import { EventMessage } from "@/types/api";
const TerminalPage = ({ messages }: { messages: Array<EventMessage> }) => {
  const [activeTab, setActiveTab] = useState(0);
  const [lastClearTimeStamp, setLastClearTimestamp] = useState<Date | null>(
    null
  );

  const handleTabChange = (event: React.SyntheticEvent, newValue: number) => {
    setActiveTab(newValue);
  };

  console.log("lastClearTimeStamp", lastClearTimeStamp);

  return (
    <Box
      className="text-black dark:text-white"
      sx={{ width: "100%", height: "100vh" }}
    >
      <Divider
        className="border-gray-300 dark:border-gray-700"
        sx={{ marginTop: "4px" }}
      />

      <AppBar
        position="static"
        className="bg-gray-200 dark:bg-[#333] border-b border-gray-300 dark:border-gray-700"
      >
        <Toolbar>
          <Tabs
            value={activeTab}
            indicatorColor="primary"
            onChange={handleTabChange}
            sx={{ flexGrow: 1 }}
            className="text-gray-700 dark:text-gray-300"
          >
            <Tab
              label="Terminal"
              className="text-gray-700 dark:text-gray-300"
            />
            <Tab
              label="Invocations"
              className="text-gray-700 dark:text-gray-300"
            />
            <Tab label="Logs" className="text-gray-700 dark:text-gray-300" />
          </Tabs>
          <Button
            variant="outlined"
            color="error"
            sx={{ marginRight: 1 }}
            className="dark:border-red-600"
            onClick={(e) => {
              e.preventDefault();
              setLastClearTimestamp(new Date());
            }}
          >
            Clear
          </Button>
          <Button
            variant="contained"
            color="primary"
            onClick={(e) => {
              e.preventDefault();
              setLastClearTimestamp(null);
            }}
          >
            Reload
          </Button>
        </Toolbar>
      </AppBar>

      <Box
        sx={{ flex: 1, mt: 2, overflowY: "auto" }}
        className="text-gray-700 dark:text-gray-300"
      >
        {activeTab === 0 && (
          <TerminalLogs lastClearTimeStamp={lastClearTimeStamp} messages={messages}/>
        )}
        {/* It is just logs for invocation only. but the console shows different. it merging the both inovked and 
        invoked completed data and showing data. need to rework on this(little tricky)*/}
        {activeTab === 1 && (
          <InvocationLogs lastClearTimeStamp={lastClearTimeStamp} messages={messages}/>
        )}
        {/* this i am not able to view in console.*/}
        {activeTab === 2 && <Logs />}
      </Box>
    </Box>
  );
};

export default TerminalPage;

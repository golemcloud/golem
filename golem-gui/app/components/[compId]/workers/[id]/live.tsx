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
import { Button2 } from "@/components/ui/button";
import TerminalLogs from "./terminal";
const TerminalPage = ({ messages }: { messages: Array<any> }) => {
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
      <Box className="flex border-b border-t">
        <Tabs
          value={activeTab}
          onChange={handleTabChange}
          sx={{
            flexGrow: 1,
            "& .MuiTab-root": {
             
              "&:hover": {
                backgroundColor: "#fff",
                color: "#000",
              },
            },
          }}
          textColor="inherit"
        >
          <Tab label="Terminal" />
          <Tab label="Invocations" />
          <Tab label="Logs" />
        </Tabs>
        <Button2
          variant="error"
          sx={{ marginRight: 1 }}
          onClick={(e) => {
            e.preventDefault();
            setLastClearTimestamp(new Date());
          }}
        >
          Clear
        </Button2>
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
      </Box>
      <Box
        sx={{ flex: 1, mt: 2, overflowY: "auto" }}
        className="text-gray-700 dark:text-gray-300"
      >
        {activeTab === 0 && (
          <TerminalLogs
            lastClearTimeStamp={lastClearTimeStamp}
            messages={messages}
          />
        )}
        {/* It is just logs for invocation only. but the console shows different. it merging the both inovked and 
        invoked completed data and showing data. need to rework on this(little tricky)*/}
        {activeTab === 1 && (
          <InvocationLogs
            lastClearTimeStamp={lastClearTimeStamp}
            messages={messages}
          />
        )}
        {/* this i am not able to view in console.*/}
        {activeTab === 2 && <Logs />}
      </Box>
    </Box>
  );
};

export default TerminalPage;

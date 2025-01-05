import React, { useState } from "react";
import {
  Box,
  Tabs,
  Tab,
} from "@mui/material";
import Logs from "./logs";
import InvocationLogs from "./invoke-logs";
import { Button2 } from "@/components/ui/button";
import TerminalLogs from "./terminal";
import { RotateCw, Ban } from "lucide-react";
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
      sx={{ width: "100%", minHeight: "100vh" }}
    >
      <Box className="flex border-b pb-1 border-t dark:bg-[#151414 justify-between items-center flex-col sm:flex-row">
        <Box className="mt-1 mb-1 dark:bg-[#1e1e1e] rounded-md mx-2">
          <Tabs
            value={activeTab}
            onChange={handleTabChange}
            sx={{
              flexGrow: 1,
              // padding: 1,
              "& .MuiTab-root": {
              textTransform: "none",
              fontSize: "0.75rem", 
              borderRadius: "2px",
              width:'fit-content',
              },
              
              "& .MuiTabs-indicator": {
                display: "none",
              },
            }}
            textColor="inherit"
          >
            <Tab
              className={`${
                activeTab === 0 ? " dark:bg-[#373737] bg-[#C0C0C0]" : ""
              }`}
              label="Terminal"
            />
            <Tab
              className={`${
                activeTab === 1 ? "dark:bg-[#373737] bg-[#C0C0C0]" : ""
              }`}
              label="Invocations"
            />
            <Tab
              className={`${
                activeTab === 2 ? "dark:bg-[#373737] bg-[#C0C0C0]" : ""
              }`}
              label="Logs"
            />
          </Tabs>
        </Box>
        <Box className="flex gap-3 mx-2">
          <Button2
            variant="error"
            size="md"
            endIcon={<Ban size="22px" />}
            onClick={(e) => {
              e.preventDefault();
              setLastClearTimestamp(new Date());
            }}
          >
            Clear
          </Button2>
          <Button2
            variant="primary"
            size="md"
            endIcon={<RotateCw size="22px" />}
            onClick={(e) => {
              e.preventDefault();
              setLastClearTimestamp(null);
            }}
          >
            Reload
          </Button2>
        </Box>
      </Box>
      <Box
        sx={{ flex: 1, mt: 2,paddingX:5}}
        className="text-gray-700 dark:text-gray-300"
      >
        {activeTab === 0 && (
          <TerminalLogs
            lastClearTimeStamp={lastClearTimeStamp}
            messages={messages}
          />
        )}
        {activeTab === 1 && (
          <InvocationLogs
            lastClearTimeStamp={lastClearTimeStamp}
            messages={messages}
          />
        )}
        {activeTab === 2 && <Logs />}
      </Box>
    </Box>
  );
};

export default TerminalPage;

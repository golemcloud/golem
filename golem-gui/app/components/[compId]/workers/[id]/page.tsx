"use client";

import React, { useMemo, useState } from "react";
import { Box, Button, Typography, Tab, Tabs, Grid, Paper } from "@mui/material";
import { useWorker } from "@/lib/hooks/use-worker";
import { useParams } from "next/navigation";
import {
  CheckCircleOutline,
  ErrorOutline,
  RocketLaunch,
} from "@mui/icons-material";
import InvokePage from "./invoke";
import useComponents from "@/lib/hooks/use-component";
import Overview from "./overview";
import TerminalPage from "./live";

const WorkerListWithDropdowns = () => {
  const [activeTab, setActiveTab] = useState(0);

  //TO DO: let show filters in url so that user can share the url to others.
  const { compId } = useParams<{ compId: string }>();
  const { id: workerName } = useParams<{ id: string }>();
  //need to integrate the filter logic here. and pagination or scroll on load needs to implemented or addd show more at the end on click we need to next set of data
  const { worker, isLoading } = useWorker(compId, workerName);
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
    <>
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
      {activeTab === 0 && <Overview worker={worker} isLoading={isLoading}/>}
      {activeTab === 5 && <InvokePage/>}
      {activeTab === 1 && <TerminalPage workerName={workerName} />}

    </>
  );
};

export default WorkerListWithDropdowns;

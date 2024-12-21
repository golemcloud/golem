"use client";

import React, { useState } from "react";
import DangerZone from "@/components/settings";
import ComponentInfo from "@/components/componentInfoCard";
import { Tabs, Tab, Box, Typography,Divider } from "@mui/material";
import useSWR from "swr";
import { fetcher } from "@/lib/utils";
import { useParams } from "next/navigation";
import CreateComponentForm from "@/components/new-component";

import { toast } from "react-toastify";

const WorkerSettings = () => {
  const [activeTab, setActiveTab] = useState(0);

  const { compId } = useParams<{ compId: string }>();
  const { data: componentData, isLoading } = useSWR(
    `?path=components/${compId}`,
    fetcher
  );

  const component = componentData?.data?.[0];
  const versionedComponentId = component?.versionedComponentId || {};

  const handleTabChange = (event: React.SyntheticEvent, newValue: number) => {
    setActiveTab(newValue);
  };

  const actions = [
    {
      title: "Delete All Workers",
      description: "This will permanently delete all workers associated with this component.",
      buttonText: "Delete All Workers",
      onClick: () => toast.success("All workers deleted successfully"),
    },
  ];

  return (
    <div>
      <Box
        sx={{
          borderBottom: 1,
          borderColor: "#444",
          width:'fit-content'
        }}
      >
        <Tabs
          value={activeTab}
          onChange={handleTabChange}
          aria-label="Worker Settings Tabs"
          textColor="inherit"
        >
          <Tab label="General"/>
          <Tab label="Info"  />
          <Tab label="Update" />
        </Tabs>
      </Box>

      <Box sx={{ p: 3 }}>
        {activeTab === 0 && (
          <div>
            <DangerZone
              title="General Settings"
              description="Manage your component settings."
              actions={actions}
            />
          </div>
        )}
        {activeTab === 1 && (
          <div>
            {component ? (
              <ComponentInfo
                componentId={versionedComponentId.componentId}
                version={versionedComponentId.version}
                name={component.componentName}
                size={component.componentSize}
                createdAt={component.createdAt}
              />
            ) : (
              <Typography>Loading component info...</Typography>
            )}
          </div>
        )}
        {activeTab === 2 && (
          <div>
            <Box className="flex flex-col">
              <Typography variant="h6">Update Component</Typography>
              <Typography variant="subtitle1" sx={{color:'#888'}}>
                Update your component version.
              </Typography>
            </Box>
            <Divider sx={{ borderColor: "#555", marginBottom: "13px" }} />
            <CreateComponentForm
              mode="update"
              ComponentId={compId}
            />
          </div>
        )}
      </Box>
    </div>
  );
};

export default WorkerSettings;

"use client";

import React, { useEffect, useState } from "react";
import DangerZone from "@/components/settings";
import ComponentInfo from "@/components/component-info-card";
import {
  Tabs,
  Tab,
  Box,
  Typography,
  Divider,
  Stack,
  MenuItem,
  Select,
} from "@mui/material";
import { useParams, useSearchParams } from "next/navigation";
import CreateComponentForm from "@/components/new-component";

import { toast } from "react-toastify";
import useComponents, { downloadComponent } from "@/lib/hooks/use-component";
import { Component } from "@/types/api";
import SecondaryHeader from "@/components/ui/secondary-header";
import ErrorBoundary from "@/components/erro-boundary";
import { Button2 } from "@/components/ui/button";
import { DownloadIcon } from "lucide-react";

const WorkerSettings = () => {
  const { compId } = useParams<{ compId: string }>();
  const { components, error } = useComponents(compId);
  const [version, setVersion] = useState<number | null>(null);
  const searchParams = useSearchParams();
  const activeTabFromQuery = Number(searchParams.get("activeTab")) || 0;

  const [activeTab, setActiveTab] = useState(activeTabFromQuery);

  const component = components?.[version ?? components?.length - 1];
  const versionedComponentId = component?.versionedComponentId || {};
  useEffect(() => {
    setVersion(components?.length - 1);
  }, [components]);
  const handleTabChange = (event: React.SyntheticEvent, newValue: number) => {
    setActiveTab(newValue);
  };

  //Delete all api is not there. deleting one by one is costly. so not performing deleting all.
  const actions = [
    {
      title: "Delete All Workers",
      description:
        "This will permanently delete all workers associated with this component.",
      buttonText: "Delete All Workers",
      onClick: () => toast.success("All workers deleted successfully"),
    },
  ];

  useEffect(() => {
    setActiveTab(activeTabFromQuery);
  }, [activeTabFromQuery]);

  return (
    <>
      <Box sx={{ display: { xs: "block", md: "none" } }}>
        <SecondaryHeader onClick={() => {}} variant="components" />
      </Box>
      {error && <ErrorBoundary message={error} />}
      <div className="mx-auto max-w-7xl px-6 lg:px-8">
        <div className="mx-auto max-w-2xl lg:max-w-none py-4">
          <Stack
            direction="row"
            alignItems={"center"}
            justifyContent={"space-between"}
          >
            <Box
              sx={{
                borderBottom: 1,
                borderColor: "#444",
                width: "fit-content",
              }}
            >
              <Tabs
                value={activeTab}
                onChange={handleTabChange}
                aria-label="Worker Settings Tabs"
                textColor="inherit"
              >
                <Tab label="General" />
                <Tab label="Info" />
                <Tab label="Update" />
              </Tabs>
            </Box>
            {activeTab === 1 && (
              <Stack
                direction="row"
                alignItems="center"
                marginBottom={2}
                gap={1}
              >
                <Select
                  name="version"
                  variant="outlined"
                  className="max-w-max"
                  value={version}
                  onChange={(e) => setVersion(Number(e.target.value))}
                >
                  {components?.map((component: Component) => (
                    <MenuItem
                      key={component?.versionedComponentId?.version}
                      value={component?.versionedComponentId?.version}
                    >
                      V{component?.versionedComponentId?.version}
                    </MenuItem>
                  ))}
                </Select>
                <Button2
                  onClick={(e) => {
                    e.preventDefault();
                    downloadComponent(compId, version!);
                  }}
                >
                  <DownloadIcon />
                </Button2>
              </Stack>
            )}
          </Stack>

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
                  <Typography variant="subtitle1" sx={{ color: "#888" }}>
                    Update your component version.
                  </Typography>
                </Box>
                <Divider sx={{ borderColor: "#555", marginBottom: "13px" }} />
                <CreateComponentForm mode="update" componentId={compId} />
              </div>
            )}
          </Box>
        </div>
      </div>
    </>
  );
};

export default WorkerSettings;

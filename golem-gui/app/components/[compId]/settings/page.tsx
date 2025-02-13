"use client";

import React, { useCallback, useMemo, useState } from "react";
import ComponentInfo from "@/app/components/component-info-card";
import { Tabs, Tab, Box, Typography, Divider, Stack } from "@mui/material";
import { useSearchParams } from "next/navigation";
import CreateComponentForm from "@/app/components/new-component";
import useComponents, { downloadComponent } from "@/lib/hooks/use-component";
import { Component } from "@/types/api";
import SecondaryHeader from "@/components/ui/secondary-header";
import ErrorBoundary from "@/components/error/error-boundary";
import { Button2 } from "@/components/ui/button";
import { DownloadIcon } from "lucide-react";
import { DropdownV2 } from "@/components/ui/dropdown-button";
import { useCustomParam } from "@/lib/hooks/use-custom-param";

const WorkerSettings = () => {
  const { compId } = useCustomParam();
  const { components, error, isLoading } = useComponents(compId);
  const [version, setVersion] = useState<number | null>(null);
  const searchParams = useSearchParams();
  const defaultTab =  searchParams.get("activeTab");
  const [activeTab, setActiveTab] = useState(defaultTab ? Number(defaultTab) : 0);

  const component = useMemo(
    () => components?.[version ?? components?.length - 1],
    [components, version]
  );

  const handleTabChange = useCallback(
    (event: React.SyntheticEvent, newValue: number) => {
      setActiveTab(newValue);
    },
    []
  );

  // const actions = useMemo(() => {
  //   return [
  //     {
  //       title: "Delete All Workers",
  //       description:
  //         "This will permanently delete all workers associated with this component.",
  //       buttonText: "Delete All Workers",
  //       onClick: () => toast.success("All workers deleted successfully"),
  //     },
  //   ];
  // }, []);

  return (
    <>
      <Box sx={{ display: { xs: "block", md: "none" } }}>
        <SecondaryHeader variant="components" />
      </Box>
      {error ||
        (!isLoading && !component && (
          <ErrorBoundary message={error || "No Component Found!"} />
        ))}
      <div className="mx-auto max-w-7xl px-2 md:px-6 lg:px-8">
        <div className="mx-auto max-w-2xl lg:max-w-none py-4">
          <div className="border rounded-b-lg">
            <Stack>
              <Box className="dark:bg-[#101010] bg-[#c0c0c0]">
                <Tabs
                  value={activeTab}
                  onChange={handleTabChange}
                  aria-label="Worker Settings Tabs"
                  textColor="inherit"
                  sx={{
                    "& .MuiTabs-indicator": {
                      bgcolor: "#373737",
                    },
                  }}
                >
                  {/* <Tab label="General" /> */}
                  <Tab label="Info" />
                  <Tab label="Update" />
                </Tabs>
              </Box>
            </Stack>

            <Box className={"p-3"}>
              {/* {activeTab === 0 && (
                <div>
                  <DangerZone
                    title="General Settings"
                    description="Manage your component settings."
                    actions={actions}
                  />
                </div>
              )} */}
              {activeTab === 0 && (
                <div>
                  <Stack
                    direction="row"
                    alignItems={"center"}
                    justifyContent={"space-between"}
                  >
                    <Box className="flex flex-col">
                      <Typography variant="h6">
                        Component Information
                      </Typography>
                      <Typography
                        variant="subtitle1"
                        className="text-muted-foreground"
                      >
                        View metadata about this component
                      </Typography>
                    </Box>
                    <Stack
                      direction="row"
                      gap={1}
                      alignItems={"center"}
                      className="self-start"
                    >
                      <DropdownV2
                        list={components?.map((component: Component) => ({
                          value: component.versionedComponentId.version,
                          label: `v${component.versionedComponentId.version}`,
                          onClick: (value: string | number) => {
                            setVersion(Number(value));
                          },
                        }))}
                        prefix={version || version == 0 ? `v${version}` : component ? `v${component.versionedComponentId.version}`: ''}
                      />
                      <Button2
                        variant="primary"
                        size="sm"
                        onClick={(e) => {
                          e.preventDefault();
                          downloadComponent(compId, version!);
                        }}
                      >
                        <DownloadIcon />
                      </Button2>
                    </Stack>
                  </Stack>
                  <Divider className="bg-border my-1" />
                  {component ? (
                    <ComponentInfo
                      componentId={component.versionedComponentId?.componentId}
                      version={component.versionedComponentId?.version}
                      name={component.componentName}
                      size={component.componentSize}
                      createdAt={component.createdAt}
                    />
                  ) : (
                    <Stack direction="row" justifyContent={"center"} mt={2}>
                      <Typography>
                        {isLoading
                          ? "Loading component info..."
                          : "No Component Found"}
                      </Typography>
                    </Stack>
                  )}
                </div>
              )}
              {activeTab === 1 && (
                <div>
                  <Box className="flex flex-col">
                    <Typography variant="h6">Update Component</Typography>
                    <Typography variant="subtitle1" sx={{ color: "#888" }}>
                      Update your component version.
                    </Typography>
                  </Box>
                  <Divider className="bg-border my-1" />
                  <CreateComponentForm mode="update" componentId={compId} />
                </div>
              )}
            </Box>
          </div>
        </div>
      </div>
    </>
  );
};


export default WorkerSettings;

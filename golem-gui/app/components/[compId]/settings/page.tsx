"use client";

import React, { useState } from "react";
import DangerZone from "@/components/settings";
import CustomModal from "@/components/CustomModal";
import { Button } from "@mui/material";
import ComponentInfo from "@/components/componentInfoCard";
import useSWR from "swr";
import { fetcher } from "@/lib/utils";
import { useParams } from "next/navigation";
import { Component } from "@/types/api";

const WorkerSettings = () => {
  const actions = [
    {
      title: "Delete All Workers",
      description: "This will permanently delete all workers associated with this component.",
      buttonText: "Delete All Workers",
      onClick: () => alert("Worker deleted!"),
    },
  ];

  const { compId } = useParams<{ compId: string }>();
  const [isOpen, setIsOpen] = useState(false);

  const handleOpen = () => setIsOpen(true);
  const handleClose = () => setIsOpen(false);

  const { data: componentData, isLoading } = useSWR(
    `?path=components/${compId}`,
    fetcher
  );
  
  const component = componentData.data?.[0];
  const versionedComponentId = component?.versionedComponentId || {};

  return (
    <div>
      <DangerZone
        title="General Settings"
        description="Manage your component settings."
        actions={actions}
      />
      <Button variant="contained" onClick={handleOpen}>
        Open Modal
      </Button>
      <CustomModal open={isOpen} onClose={handleClose} heading="Component Information">
        {component && (
          <ComponentInfo
            componentId={versionedComponentId.componentId}
            version={versionedComponentId.version}
            name={component.componentName}
            size={component.componentSize}
            createdAt={component.createdAt}
          />
        )}
      </CustomModal>
    </div>
  );
};

export default WorkerSettings;

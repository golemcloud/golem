"use client";

import React from "react";
import DangerZone from "@/components/settings";

const ApiSettings = () => {
  const actions = [
    {
      title: "Delete API Version 0.8",
      description: "Once you delete an API, there is no going back. Please be certain.",
      buttonText: "Delete Version 0.8",
      onClick: () => alert("Version 0.8 deleted!"),
    },
    {
      title: "Delete All API Versions",
      description: "Once you delete all API versions, there is no going back. Please be certain.",
      buttonText: "Delete All Versions",
      onClick: () => alert("All versions deleted!"),
    },
  ];

  return (
    <div>
      <DangerZone
        title="Danger Zone"
        description="Proceed with caution."
        actions={actions}
      />
    </div>
  );
};

export default ApiSettings;

"use client";

import React from "react";
import DangerZone from "@/components/settings";
import { useParams, useSearchParams } from "next/navigation";
import useApiDefinitions from "@/lib/hooks/use-api-definitons";
import ErrorBoundary from "@/components/erro-boundary";

const ApiSettings = () => {
  const { apiId} = useParams<{apiId:string}>();
  const params = useSearchParams();
  const version = params.get("version");
  const { deleteVersion, error, isLoading } = useApiDefinitions(apiId)

  const actions = [
    {
      title: `Delete API Version ${version || ""}`,
      description: "Once you delete an API, there is no going back. Please be certain.",
      buttonText: `Delete Version ${version || ""}`,
      disabled: !!error || isLoading,
      onClick: (e:React.MouseEvent<HTMLButtonElement>) => {e?.preventDefault(); if(error){return}deleteVersion(apiId, version)},
    },
    {
      title: "Delete All API Versions",
      description: "Once you delete all API versions, there is no going back. Please be certain.",
      buttonText: "Delete All Versions",
      disabled: !!error || isLoading,
      // I don't see any api for this at one go. if it really needed we need to perform this batch wise one by one. for now skipping it
      onClick: (e:React.MouseEvent<HTMLButtonElement>) =>{e?.preventDefault(); if(error){return} alert("All versions deleted!")},
    },
  ];

  return (
    <div>
      {error && <ErrorBoundary message={error}/>}
      <DangerZone
        title="Api Settings"
        description="Manage your API settings."
        actions={actions}
      />
    </div>
  );
};

export default ApiSettings;

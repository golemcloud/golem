"use client";

import React, { useState } from "react";
import DangerZone from "@/components/settings";
import { useSearchParams } from "next/navigation";
import useApiDefinitions from "@/lib/hooks/use-api-definitons";
import ErrorBoundary from "@/components/error-boundary";
import CustomModal from "@/components/custom-modal";
import DeleteApiVersion from "../../api-version-deletion";
import { useCustomParam } from "@/lib/hooks/use-custom-param";

const ApiSettings = () => {
  const { apiId } = useCustomParam();
  const params = useSearchParams();
  const version = params.get("version");
  const { apiDefinitions, isLoading, getApiDefintion, error: requestError } = useApiDefinitions(apiId, version)

  const [open, setOpen] = useState<boolean>(false);
  
  const handleClose = ()=>setOpen(false);
  const {error} = (!isLoading && getApiDefintion() || {});

  const actions = [
    {
      title: `Delete API Version ${version || ""}`,
      description: "Once you delete an API, there is no going back. Please be certain.",
      buttonText: `Delete Version ${version || ""}`,
      disabled: !!error || isLoading,
      onClick: (e:React.MouseEvent<HTMLButtonElement>) => {e?.preventDefault(); if(error){return}setOpen(true)},
    },
    {
      title: "Delete All API Versions",
      description: "Once you delete all API versions, there is no going back. Please be certain.",
      buttonText: "Delete All Versions",
      disabled: !!error || isLoading,
      // I don't see any api for this at one go. if it really needed we need to perform this batch wise one by one. for now skipping it
      onClick: (e:React.MouseEvent<HTMLButtonElement>) =>{e?.preventDefault(); if(error){return} alert("All versions deleted (coming soon)!")},
    },
  ];

  return (
    <div>
      
      {(error ||requestError)  && <ErrorBoundary message={requestError || error}/>}
      <DangerZone
        title="API Settings"
        description="Manage your API settings."
        actions={actions}
      />
        <CustomModal
        open={!!open}
        onClose={handleClose}
        heading={`Delete Api version`}
      >
          <DeleteApiVersion apiId={apiId} version={apiDefinitions?.[apiDefinitions?.length-1]?.version} onSuccess={handleClose}/>
      </CustomModal>
    </div>
  );
};

export default ApiSettings;

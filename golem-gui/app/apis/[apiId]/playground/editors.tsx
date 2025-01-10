"use client";
import React, { useEffect, useState } from "react";
import useStore from "@/lib/hooks/use-react-flow-store";
import { useParams } from "next/navigation";
import NewRouteForm from "@/components/new-route";
import CustomModal from "@/components/CustomModal";
import CreateNewApiVersion from "@/components/create-api-new-version";
import DeleteApiVersion from "@/components/api-version-deletion";
import { downloadApi } from "@/lib/hooks/use-api-definitons";
import { Typography } from "@mui/material";
import JsonEditor from "@/components/json-editor";
import DeploymentCreationPage from "@/components/deployment-creation";

const operationMap = {
  "new_route": "New Route",
  "new_api": "New Version",
  "update_api": "Update Api",
  "delete_api": "Delete Api",
  "download_api": "Download Api",
  "update_route": "Update Route",
  "delete_route": "Delete Route",
} as Record<string, string>

export default function Editors() {
  const [open, setOpen] = useState<string | null>(null);
  const { apiId } = useParams<{ apiId: string }>();
  // const params = useSearchParams();
  // const version = params.get("version");
  const { trigger, setTrigger, setSelectedNode, setSelectedEdge, nodes } =
    useStore();
  const handleClose = () => {
    setTrigger(null);
    setSelectedNode(null);
    setSelectedEdge(null);
  };

  useEffect(() => {
    (async () => {
      if (trigger && trigger.operation === "download_api") {
        setOpen(trigger?.type || null);
        switch (trigger.type) {
          case "api":
            await downloadApi(apiId, trigger?.meta?.version);
            break;
          default: //do nothing
        }
        return handleClose();
      }
      setOpen(trigger?.type || null);
    })();
  }, [apiId, trigger]);

  return (
    <div>
      <CustomModal
        open={open == null ? false : true}
        onClose={handleClose}
        heading={`${
          trigger?.operation
            ? operationMap[trigger.operation] || trigger.operation.charAt(0).toUpperCase() +
              trigger.operation.slice(1)
            : "Default Operation"
        } ${trigger?.type === "route" ? "Route" : "API"}`}
      >
        <>
          {trigger?.type === "api" && (
            <>
              {trigger?.operation === "new_route" && (
                 <NewRouteForm
                 apiId={apiId}
                 onSuccess={handleClose}
                 version={trigger?.meta?.version}
                 isModal={true}
               />
              )}
              {trigger?.operation === "new_api" && (
                <CreateNewApiVersion
                  onSuccess={({ version }: { version: string }) => {
                    handleClose();
                    setSelectedNode(`${apiId}__${version}__api`);
                  }}
                  apiId={apiId}
                  version={trigger?.meta?.version}
                  noRedirect={true}
                />
              )}
              {trigger?.operation === "delete_api" && (
                <DeleteApiVersion
                  apiId={apiId}
                  version={trigger?.meta?.version}
                  noRedirect={true}
                  onSuccess={handleClose}
                />
              )}
              {trigger?.operation === "download_api" && (
                <Typography>Downloading....</Typography>
              )}
              {trigger?.operation === "view_api" && (
                <JsonEditor
                  json={
                    nodes?.find(
                      (node) =>
                        `${apiId}__${trigger?.meta?.version}__api` === node.id
                    )?.data || "No Api Found!"
                  }
                />
              )}
              {trigger?.operation === "deploy_api" && (
                <DeploymentCreationPage
                  apiId={apiId}
                  version={trigger?.meta?.version}
                />
              )}
            </>
          )}
          {trigger?.type === "route" && (
            <>
             {trigger?.operation === "new_route" && (
                 <NewRouteForm
                 apiId={apiId}
                 onSuccess={handleClose}
                 version={trigger?.meta?.version}
                 isModal={true}
               />
              )}
              {trigger?.operation === "delete_route" && (
                // Chnage it to delete modal. work in progress
                <NewRouteForm
                  apiId={apiId}
                  onSuccess={handleClose}
                  defaultRoute={trigger?.meta?.route}
                  // isExperimental={true}
                  version={trigger?.meta?.version}
                  isModal={true}
                />
              )}
              {trigger?.operation === "update_route" && (
                // Chnage it to update modal. work in progress
                <NewRouteForm
                  apiId={apiId}
                  onSuccess={handleClose}
                  defaultRoute={trigger?.meta?.route}
                  version={trigger?.meta?.version}
                  isModal={true}
                />
              )}
              {trigger?.operation === "view_route" && (
                // Chnage it to view modal. work in progress
                <NewRouteForm
                  apiId={apiId}
                  onSuccess={handleClose}
                  isExperimental={true}
                  isModal={true}
                />
              )}
            </>
          )}
        </>
      </CustomModal>
    </div>
  );
}

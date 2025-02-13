"use client";
import React, { useEffect, useState } from "react";
import useStore from "@/lib/hooks/use-react-flow-store";
import NewRouteForm from "../../new-route";
import CustomModal from "@/components/custom/custom-modal";
import CreateNewApiVersion from "../../create-api-new-version";
import DeleteApiVersion from "../../api-version-deletion";
import { downloadApi } from "@/lib/hooks/use-api-definitons";
import { Typography } from "@mui/material";
import JsonEditor from "@/components/json-editor/json-editor";
import DeploymentCreationPage from "@/app/apis/deployment-creation";
const operationMap = {
  "new_route": "New Route",
  "new_api": "New Version",
  "update_api": "Update Api",
  "delete_api": "Delete Api",
  "download_api": "Download Api",
  "update_route": "Update Route",
  "delete_route": "Delete Route",
  "view_route": "View Route",
  "view_api": "View Api",
} as Record<string, string>
import { useCustomParam } from "@/lib/hooks/use-custom-param";
import RouteDetails from "../[routeId]/route-details";

export default function Editors() {
  const [open, setOpen] = useState<string | null>(null);
  const { apiId } = useCustomParam();
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
  }, [apiId, trigger,handleClose]);

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
                 noRedirect={true}
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
                 noRedirect={true}
               />
              )}
              {trigger?.operation === "update_route" && (
                <NewRouteForm
                  apiId={apiId}
                  onSuccess={handleClose}
                  defaultRoute={trigger?.meta?.route}
                  version={trigger?.meta?.version}
                  isModal={true}
                  noRedirect={true}
                />
              )}
              {trigger?.operation === "view_route" && (
                <RouteDetails
                  defaultRouteId={`${trigger.meta && `${trigger.meta.route?.path}|${trigger.meta.route?.method}`}`}
                  defaultVersion={trigger?.meta?.version}
                  noRedirect={true}
                />
              )}
            </>
          )}
        </>
      </CustomModal>
    </div>
  );
}

"use client";
import React, { useEffect, useState } from "react";
import useStore from "@/lib/hooks/use-react-flow-store";
import { useParams, useSearchParams } from "next/navigation";
import NewRouteForm from "@/components/new-route";
import CustomModal from "@/components/CustomModal";
import CreateNewApiVersion from "@/components/create-api-new-version";
import DeleteApiVersion from "@/components/api-version-deletion";

export default function Editors() {
  const [open, setOpen] = useState<string | null>(null);
  const { apiId } = useParams<{ apiId: string }>();
  const params = useSearchParams();
  const version = params.get("version")
  const { trigger, setTrigger, setSelectedNode, setSelectedEdge } = useStore();
  const handleClose = () => {
    setTrigger(null);
    setSelectedNode(null);
    setSelectedEdge(null);
  };

  useEffect(() => {
    setOpen(trigger?.type || null);
  }, [trigger]);

  return (
    <div>
      <CustomModal
        open={open == null ? false : true}
        onClose={handleClose}
        heading={`${
          trigger?.operation
            ? trigger.operation.charAt(0).toUpperCase() +
              trigger.operation.slice(1)
            : "Default Operation"
        } ${trigger?.type === "route" ? "Route" : "API"}`}
      >
        <>
          {trigger?.type === "api" && (
            <>
              {trigger?.operation === "new_version" && (
                <CreateNewApiVersion
                  onSuccess={handleClose}
                  apiId={apiId}
                  version={version}
                  isExperimental={true}
                />
              )}
              {trigger?.operation === "create" && (
                <CreateNewApiVersion
                  onSuccess={handleClose}
                  apiId={apiId}
                  isExperimental={true}
                />
              )}
              {trigger?.operation === "delete" && (
                // Chnage it to delete modal. work in progress
                <DeleteApiVersion apiId={apiId} version={version}/>
              )}
              {/* {trigger?.operation === "update" && (
                // Chnage it to update modal. work in progress
                <CreateNewApiVersion
                  onSuccess={handleClose}
                  apiId={apiId}
                  isExperimental={true}
                />
              )} */}
              {trigger?.operation === "view" && (
                // Chnage it to view modal. work in progress
                <CreateNewApiVersion
                  onSuccess={handleClose}
                  apiId={apiId}
                  isExperimental={true}
                />
              )}
            </>
          )}
          {/* for now to differentiate we are using component. need chnge it correct creating element */}
          {trigger?.type === "route" && (
            <div className="p-5">
              {trigger?.operation === "create" && (
                <NewRouteForm
                  apiId={apiId}
                  onSuccess={handleClose}
                  isExperimental={true}
                />
              )}
              {trigger?.operation === "delete" && (
                // Chnage it to delete modal. work in progress
                <NewRouteForm
                  apiId={apiId}
                  onSuccess={handleClose}
                  isExperimental={true}
                />
              )}
              {trigger?.operation === "update" && (
                // Chnage it to update modal. work in progress
                <NewRouteForm
                  apiId={apiId}
                  onSuccess={handleClose}
                  isExperimental={true}
                />
              )}
              {trigger?.operation === "view" && (
                // Chnage it to view modal. work in progress
                <NewRouteForm
                  apiId={apiId}
                  onSuccess={handleClose}
                  isExperimental={true}
                />
              )}
            </div>
          )}
        </>
      </CustomModal>
    </div>
  );
}

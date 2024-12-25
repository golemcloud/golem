"use client";
import { Container, Modal, Paper, TextField, Typography } from "@mui/material";
import React, { useEffect, useState } from "react";
import useStore from "@/lib/hooks/use-react-flow-store";
import CreateAPI from "@/components/create-api";
import { useParams } from "next/navigation";
import NewRouteForm from "@/components/new-route";
import CustomModal from "@/components/CustomModal";
import CreateNewApiVersion from "@/components/create-api-new-version";


export default function Editors() {
  const [open, setOpen] = useState<string | null>(null);
  const { apiId } = useParams<{ apiId: string }>();
  const { trigger, setTrigger, setSelectedNode, setSelectedEdge } = useStore();
  const handleOpen = (type: string) => setOpen(type);
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
      <CustomModal open={(open==null?false:true)} onClose={handleClose} heading={`${trigger?.operation} ${(trigger?.type=='route'?"Route":"API")}`}>
        <>
          {trigger?.type === "api" && (
            <>
              {trigger?.operation === "new_version" && (
                <CreateNewApiVersion onSuccees={handleClose} apiId={apiId}  isExperimental={true} />
              )}
              {trigger?.operation === "create" && (
                <CreateNewApiVersion onSuccees={handleClose} apiId={apiId}  isExperimental={true} />
              )}
              {trigger?.operation === "delete" && (
                // Chnage it to delete modal. work in progress
                <TextField placeholder="enter api version"/>
              )}
              {trigger?.operation === "update" && (
                // Chnage it to update modal. work in progress
                <CreateAPI onCreation={handleClose} isExperimental={true} />
              )}
               {trigger?.operation === "view" && (
                // Chnage it to view modal. work in progress
                <CreateAPI onCreation={handleClose} isExperimental={true} />
              )}
            </>
          )}
          {/* for now to differentiate we are using component. need chnge it correct creating element */}
          {trigger?.type === "route" && (
              <Paper elevation={4}>
                {trigger?.operation === "create" && (
                <NewRouteForm apiId={apiId} onSuccess={handleClose} isExperimental={true} />
              )}
               {trigger?.operation === "delete" && (
                // Chnage it to delete modal. work in progress
                <NewRouteForm apiId={apiId} onSuccess={handleClose} isExperimental={true} />
              )}
              {trigger?.operation === "update" && (
                // Chnage it to update modal. work in progress
                <NewRouteForm apiId={apiId} onSuccess={handleClose} isExperimental={true}/>
              )}
               {trigger?.operation === "view" && (
                // Chnage it to view modal. work in progress
                <NewRouteForm apiId={apiId} onSuccess={handleClose} isExperimental={true} />
              )}
              </Paper>
          )}
        </>
        </CustomModal>
    </div>
  );
}

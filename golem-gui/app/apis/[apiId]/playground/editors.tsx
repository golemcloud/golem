"use client";
import { Container, Modal, Paper } from "@mui/material";
import React, { useEffect, useState } from "react";
import useStore from "@/lib/hooks/use-react-flow-store";
import CreateAPI from "@/components/create-api";
import { useParams } from "next/navigation";
import NewRouteForm from "@/components/new-route";

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
      <Modal open={!!open} onClose={handleClose}>
        <>
          {trigger?.type === "api" && (
            <>
              {trigger?.operation === "create" && (
                <CreateAPI onCreation={handleClose} />
              )}
              {trigger?.operation === "delete" && (
                // Chnage it to delete modal. work in progress
                <CreateAPI onCreation={handleClose} />
              )}
              {trigger?.operation === "update" && (
                // Chnage it to update modal. work in progress
                <CreateAPI onCreation={handleClose} />
              )}
               {trigger?.operation === "view" && (
                // Chnage it to view modal. work in progress
                <CreateAPI onCreation={handleClose} />
              )}
            </>
          )}
          {/* for now to differentiate we are using component. need chnge it correct creating element */}
          {trigger?.type === "route" && (
            <Container maxWidth="md" sx={{ mt: 4 }}>
              <Paper elevation={3} sx={{ borderRadius: 2 }}>
                {trigger?.operation === "create" && (
                <NewRouteForm apiId={apiId} onCreation={handleClose} />
              )}
               {trigger?.operation === "delete" && (
                // Chnage it to delete modal. work in progress
                <NewRouteForm apiId={apiId} onCreation={handleClose} />
              )}
              {trigger?.operation === "update" && (
                // Chnage it to update modal. work in progress
                <NewRouteForm apiId={apiId} onCreation={handleClose} />
              )}
               {trigger?.operation === "view" && (
                // Chnage it to view modal. work in progress
                <NewRouteForm apiId={apiId} onCreation={handleClose} />
              )}
              </Paper>
            </Container>
          )}
        </>
      </Modal>
    </div>
  );
}

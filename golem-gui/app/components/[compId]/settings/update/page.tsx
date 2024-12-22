"use client";
import React, { useState } from "react";
import { Container } from "@mui/material";

import CreateComponentForm from "@/components/new-component";
import { useParams, useRouter } from "next/navigation";
import useComponents from "@/lib/hooks/use-component";

const ComponentsPage = () => {
  const [open, setOpen] = useState(false);

  const handleOpen = () => setOpen(true);
  const handleClose = () => setOpen(false);
  const router = useRouter();
  const { compId } = useParams<{ compId: string }>();
  const { components } = useComponents(compId);

  return (
    <Container maxWidth="lg" sx={{ mt: 5, height: "100vh" }}>
      {/* Modal for Creating New API/Component */}
      {/* <CustomModal open={open} onClose={handleClose} heading="Create a new Component"> */}
      <CreateComponentForm
        onSubmitSuccess={() => {
          handleClose();
        }}
        mode="update"
      />
      {/* </CustomModal> */}
    </Container>
  );
};

export default ComponentsPage;

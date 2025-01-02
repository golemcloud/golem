"use client";
import React, { useState } from "react";
import { Container } from "@mui/material";

import CreateComponentForm from "@/components/new-component";

const ComponentsPage = () => {
  const [, setOpen] = useState(false);

  const handleClose = () => setOpen(false);

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

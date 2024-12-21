'use client'
import React, { useState } from "react";
import {
  Container,
} from "@mui/material";

import CreateComponentForm from "@/components/new-component"
import { Component } from "@/types/api";
import { useRouter } from "next/navigation";
import { useComponents } from "@/lib/hooks/useComponents";
import CustomModal from "@/components/CustomModal";


const ComponentsPage = () => {
  const [open, setOpen] = useState(false);

  const handleOpen = () => setOpen(true);
  const handleClose = () => setOpen(false);
  const router = useRouter();

  const { data: componentData, isLoading } = useComponents();
  const components = (componentData?.data || []) as Component[];


  return (
    <Container maxWidth="lg" sx={{ mt: 5, height: "100vh" }}>
      {/* Modal for Creating New API/Component */}
      {/* <CustomModal open={open} onClose={handleClose} heading="Create a new Component"> */}
      <CreateComponentForm
            onCreation={() => {
              handleClose();
            }}
            mode="update"
          />

      {/* </CustomModal> */}
    </Container>
  );
};

export default ComponentsPage;

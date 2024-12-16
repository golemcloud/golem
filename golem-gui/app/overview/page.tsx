"use client"

import React, { useState } from "react";
import { Button, Modal, TextField, Box, Typography } from "@mui/material";
import CreateComponentForm from "../../components/new-component";
import CreateAPI from "@/components/create-api";

const OverviewPage = () => {
  const [open, setOpen] = useState<string|null>(null);
  const [apis, setApis] = useState([]);
  const [components, setComponents] = useState([]);

  const handleOpen = (type:string) => setOpen(type);
  const handleClose = () => setOpen(null);

  return (
    <Box display="flex" height="100vh" padding={10}>
      {/* Left Box (APIs) */}
      <Box
        flex={1}
        bgcolor="#333"
        color="#fff"
        padding={2}
        borderRadius="8px"
        marginRight={2}
        display="flex"
        flexDirection="column"
      >
        <Typography variant="h6" mb={2}>
          APIs
        </Typography>

        {/* Show Create Button if no APIs */}
        {apis.length === 0 ? (
          <Box flex={1} display="flex" justifyContent="center" alignItems="center">
            <Button variant="contained" color="primary" onClick={()=>handleOpen("api")}>
              Create New API
            </Button>
          </Box>
        ) : (
          <>
            <Button
              variant="outlined"
              color="primary"
              sx={{ marginBottom: 2 }}
              onClick={()=>handleOpen("api")}
            >
              Create New API
            </Button>
            <Box>
              {/* Render APIs */}
              {apis.map((api, index) => (
                <Box key={index} bgcolor="#444" marginBottom={1} padding={1}>
                  <Typography variant="body1">{api}</Typography>
                </Box>
              ))}
            </Box>
          </>
        )}
      </Box>

      {/* Right Box (Components) */}
      <Box
        flex={1}
        bgcolor="#333"
        color="#fff"
        padding={2}
        borderRadius="8px"
        marginLeft={2}
        display="flex"
        flexDirection="column"
      >
        <Typography variant="h6" mb={2}>
          Components
        </Typography>

        {/* Show Create Button if no Components */}
        {components.length === 0 ? (
          <Box flex={1} display="flex" justifyContent="center" alignItems="center">
            <Button variant="contained" color="primary" onClick={()=>handleOpen("component")}>
              Create New Component
            </Button>
          </Box>
        ) : (
          // If Components are present, show the "Create" button at the top
          <>
            <Button
              variant="outlined"
              color="primary"
              sx={{ marginBottom: 2 }}
              onClick={()=>handleOpen("component")}
            >
              Create New Component
            </Button>
            <Box>
              {/* Render Components */}
              {components.map((component, index) => (
                <Box key={index} bgcolor="#444" marginBottom={1} padding={1}>
                  <Typography variant="body1">{component}</Typography>
                </Box>
              ))}
            </Box>
          </>
        )}
      </Box>

      {/* Modal for Creating New API/Component */}
      <Modal open={!!open} onClose={handleClose}>
        <>
        {open === "api" && <CreateAPI/>}
        {open === "component" && <CreateComponentForm/>}
        </>
      </Modal>
    </Box>
  );
};

export default OverviewPage;

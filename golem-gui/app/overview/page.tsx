"use client";
import React, { useState } from "react";
import {
  Box,
  Typography,
  Button,
  Paper,
  Grid2 as Grid,
  Stack,
  Divider,
  Alert,
} from "@mui/material";
import AddIcon from "@mui/icons-material/Add";
import FooterLinks from "@/components/ui/footer-links";
import CreateAPI from "@/components/create-api";
import CreateComponentForm from "@/components/new-component";
import { ApiDefinition } from "@/types/api";
import { useRouter } from "next/navigation";
import useApiDefinitions from "@/lib/hooks/use-api-definitons";
import useComponents from "@/lib/hooks/use-component";
import CustomModal from "@/components/CustomModal";
import ComponentCard from "../../components/component-card";
import { calculateHoursDifference, calculateSizeInMB } from "@/lib/utils";
import { NotepadText,Component,Globe ,Bot} from 'lucide-react';

const ProjectDashboard = () => {
  const router = useRouter();
  const resources = [
    {
      label: "Language Guides",
      icon: <NotepadText />,
      description:
        "Check our language and start building",
    },
    {
      label: "Components",
      icon: <Component />,
      description:
        "Create Wasm components that run on Golem", 
    },
    {
      label: "APIs",
      icon: <Globe />,
      description:
        "Craft custom APIs to expose your components to the world",
    },
    {
      label: "Workers",
      icon: <Bot />,
      description:
        "Launch and manage efficient workers from your components",
    },
  ];
  const [open, setOpen] = useState<string | null>(null);
  const { apiDefinitions, isLoading, error } = useApiDefinitions();
  const { components, isLoading: componentsLoading, error: componentError } = useComponents();
  //TODO we need limit the api we are showing in the Ui. for now we are showing all.
  const apiMap = apiDefinitions?.reduce<Record<string, ApiDefinition>>(
    (obj, api: ApiDefinition) => {
      obj[api.id] = api;
      return obj;
    },
    {}
  );

  const uniquesApis = Object.values(apiMap);

  function handleComponentClick(id: string) {
    console.log("Component Clicked");
    router.push(`/components/${id}/overview`);
  }

  // const handleOpen = (type: string) => setOpen(type);
  const handleClose = () => setOpen(null);

  return (
    <Box sx={{ minHeight: "100vh", marginTop: 4, px: { xs: 2, md: 10 } }}
     className="container mx-auto flex flex-col gap-8 px-4 py-8 md:px-6 lg:px-8"
    >
      <Grid container spacing={3} sx={{ flexWrap: "wrap" }}>
        {/* APIs Section */}
        <Grid  size={{xs:12 ,md:4}} >
          <Paper
            elevation={3}
            sx={{
              p: 2,
              minHeight: { xs: "auto", md: "calc(100vh - 120px)" },
              height: { md: "calc(100vh - 120px)" },
              borderRadius: 2,
              display: "flex",
              flexDirection: "column",
            }}
          >
            <Box className="flex justify-between">
              <Typography variant="h5">APIs</Typography>
              {uniquesApis?.length > 0 && (
                <Button
                  variant="text"
                  sx={{
                    fontSize: "0.8rem",
                    border: "0.1px solid #555",
                    textTransform: "none",
                  }}
                  className="text-[#888] dark:text-gray-400"
                  onClick={() => router.push("/apis")}
                >
                  View All
                </Button>
              )}
            </Box>
            {error && (
              <Box sx={{ display: "flex", justifyContent: "center" }}>
                {error && <Alert severity="error">{error}</Alert>}
              </Box>
            )}
            {!error && !isLoading && <Stack marginTop={2} sx={{ flex: 1, overflow: "hidden" }}>
              {!isLoading &&
                uniquesApis.slice(0, 10).map((api) => (
                  <React.Fragment key={api.id}>
                    <Divider sx={{ bgcolor: "#555" }} />
                    <Box
                      key={api.id}
                      padding={3}
                      className="hover:bg-[#444] cursor-pointer"
                      onClick={() =>
                        router.push(
                          `/apis/${api.id}/overview?version=${api.version}`
                        )
                      }
                    >
                      <Box display="flex" justifyContent="space-between">
                        <Typography variant="body1">{api.id}</Typography>
                        <Typography
                          variant="body2"
                          sx={{
                            px: 1,
                            border: "1px solid #555",
                            borderRadius: 1,
                          }}
                        >
                          {api.version}
                        </Typography>
                      </Box>
                    </Box>
                  </React.Fragment>
                ))}
            </Stack>}
          </Paper>
        </Grid>

        {/* Components Section */}
        <Grid size={{ xs:12, md:8}}>
          <Paper
            elevation={3}
            sx={{
              p: 2,
              minHeight: { xs: "auto", md: "calc(100vh - 120px)" },
              height: { md: "calc(100vh - 120px)" },
              borderRadius: 2,
              display: "flex",
              flexDirection: "column",
            }}
          >
            <Box className="flex justify-between">
              <Typography variant="h5">Components</Typography>
              {components.length > 0 && (
                <Button
                  variant="text"
                  sx={{
                    fontSize: "0.8rem",
                    border: "0.1px solid #555",
                    textTransform: "none",
                  }}
                  className="text-[#888] dark:text-gray-400"
                  onClick={() => router.push("/components")}
                >
                  View All
                </Button>
              )}
            </Box>
            {componentError && (
              <Box sx={{ display: "flex", justifyContent: "center" }}>
                {error && <Alert severity="error">{componentError}</Alert>}
              </Box>
            )}
            {!componentError && !componentsLoading &&<Box
              sx={{
                mt: 2,
                gap: 2,
                display: "flex",
                flexWrap: "wrap",
                // justifyContent: "center",
                flex: 1, // Ensures it stretches within its parent
                overflow: "hidden", // Prevents scrolling
              }}
            >
              {components
                  .slice(0, 6)
                  .map((component) => (
                    <ComponentCard
                      key={component.versionedComponentId.componentId}
                      name={component.componentName}
                      time={calculateHoursDifference(component.createdAt)}
                      version={component.versionedComponentId.version}
                      exports={component.metadata.exports.length}
                      size={calculateSizeInMB(component.componentSize)}
                      type={component.componentType}
                      onClick={() => handleComponentClick(component.versionedComponentId.componentId!)}
                    />
                  ))}
              {components.length === 0 && (
                <Box
                  textAlign="center"
                  sx={{
                    borderRadius: 2,
                    border: "2px dashed #444",
                    py: 6,
                    px: 2,
                  }}
                >
                  <Typography variant="h6" color="text.secondary">
                    No Project Components
                  </Typography>
                  <Typography variant="body2" color="text.secondary">
                    Create your first component to get started
                  </Typography>
                  <Button
                    variant="contained"
                    startIcon={<AddIcon />}
                    sx={{
                      mt: 2,
                      bgcolor: "#444",
                      "&:hover": { bgcolor: "#555" },
                    }}
                  >
                    Create New
                  </Button>
                </Box>
              )}
            </Box>}
          </Paper>
        </Grid>
      </Grid>

      <CustomModal open={!!open} onClose={handleClose}>
        {open === "api" && <CreateAPI onCreation={handleClose} />}
        {open === "component" && (
          <CreateComponentForm mode="create" onSubmitSuccess={handleClose} />
        )}
      </CustomModal>
      <FooterLinks variant="others" resources={resources}/>
    </Box>
  );
};

export default ProjectDashboard;

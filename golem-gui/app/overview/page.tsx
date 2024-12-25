"use client";
import React, { useState } from "react";
import {
  Box,
  Typography,
  Button,
  Paper,
  Grid,
  Stack,
  Divider,
} from "@mui/material";
import AddIcon from "@mui/icons-material/Add";
import OverviewFooter from "@/components/ui/overview-footer";
import CreateAPI from "@/components/create-api";
import CreateComponentForm from "@/components/new-component";
import { fetcher } from "@/lib/utils";
import { ApiDefinition, Component } from "@/types/api";
import useSWR from "swr";
import { useRouter } from "next/navigation";
import useApiDefinitions from "@/lib/hooks/use-api-definitons";
import useComponents from "@/lib/hooks/use-component";
import CustomModal from "@/components/CustomModal";
import ComponentCard from "../../components/component-card";
import { calculateHoursDifference, calculateSizeInMB } from "@/lib/utils";

const ProjectDashboard = () => {
  const router = useRouter();
  const [open, setOpen] = useState<string | null>(null);
  const { apiDefinitions, isLoading } = useApiDefinitions();
  const { components, isLoading: componentsLoading } = useComponents();
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
    <Box sx={{ minHeight: "100vh", marginTop: 4, px: { xs: 2, md: 10 } }}>
      <Grid container spacing={3} sx={{ flexWrap: "wrap" }}>
        {/* APIs Section */}
        <Grid item xs={12} md={4}>
          <Paper
            elevation={3}
            sx={{
              p: 2,
              minHeight: { xs: "auto", md: "calc(100vh - 120px)" },
              height: { md: "calc(100vh - 120px)" }, // Ensures height consistency
              borderRadius: 2,
              display: "flex",
              flexDirection: "column", // For stacking items within
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
            <Stack marginTop={2} sx={{ flex: 1, overflow: "hidden" }}>
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
            </Stack>
          </Paper>
        </Grid>

        {/* Components Section */}
        <Grid item xs={12} md={8}>
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
            <Box
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
              {!componentsLoading &&
                components
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
              {!componentsLoading && components.length === 0 && (
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
            </Box>
          </Paper>
        </Grid>
      </Grid>

      <CustomModal open={!!open} onClose={handleClose}>
        {open === "api" && <CreateAPI onCreation={handleClose} />}
        {open === "component" && (
          <CreateComponentForm mode="create" onSubmitSuccess={handleClose} />
        )}
      </CustomModal>
      <OverviewFooter />
    </Box>
  );
};

export default ProjectDashboard;

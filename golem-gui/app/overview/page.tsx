"use client";
import React, { useState } from "react";
import {
  Box,
  Typography,
  Button,
  Paper,
  Grid2,
  Modal,
  Stack,
} from "@mui/material";
import AddIcon from "@mui/icons-material/Add";
import OverviewFooter from "@/components/ui/overview-footer";
import CreateAPI from "@/components/create-api";
import CreateComponentForm from "@/components/new-component";
import { fetcher } from "@/lib/utils";
import { ApiDefinition , Component} from "@/types/api";
import useSWR from "swr";
import { useRouter } from "next/navigation";

const ProjectDashboard = () => {
  const router = useRouter();
  const [open, setOpen] = useState<string | null>(null);
  const { data: apiData, isLoading } = useSWR("?path=api/definitions", fetcher);
  //TODO
  const { data: componentData, isLoading: componentsLoading } = useSWR(
    "?path=components",
    fetcher
  );
  const apis = (apiData?.data || []) as ApiDefinition[];
  const components = (componentData?.data || []) as Component[];
  //TODO we need limit the api we are showing in the Ui. for now we are showing all.
  const apiMap = apis?.reduce<
    Record<string, { versions: ApiDefinition[]; latestVersion: ApiDefinition }>
  >((obj, api: ApiDefinition) => {
    if (api.id in obj) {
      obj[api.id].versions.push(api);
      obj[api.id].latestVersion = api;
    } else {
      obj[api.id] = {
        versions: [api] as ApiDefinition[],
        latestVersion: api,
      };
    }
    return obj;
  }, {});

  const uniquesApis = Object.values(apiMap);

  const handleOpen = (type: string) => setOpen(type);
  const handleClose = () => setOpen(null);

  return (
    <Box sx={{ minHeight: "100vh", marginTop: "2rem" }} px={10}>
      <Grid2 container spacing={3}>
        {/* APIs Section */}
        <Grid2 size={4}>
          <Paper
            elevation={3}
            sx={{
              p: 2,
              height: "calc(100vh - 120px)",
              borderRadius: 2,
              // border: '1px solid  #999',
              position: "relative",
            }}
          >
            {/* View All Button */}
            {uniquesApis.slice(10)?.length>0 && <Button
              variant="text"
              sx={{
                position: "absolute",
                top: 8,
                right: 8,
                fontSize: "0.8rem",
                border: "0.1px solid #555",
                textTransform: "none",
              }}
              className="text-[#888] dark:text-gray-400"
              onClick={()=>{router.push('/apis')}}
            >
              View All
            </Button>}
            <Button
                variant="contained"
                startIcon={<AddIcon />}
                sx={{
                  backgroundColor: "#444",
                  color: "white",
                  "&:hover": { backgroundColor: "#555" },
                }}
                onClick={()=>handleOpen("api")}
              >
                Create New
              </Button>
            <Stack marginTop={6}>
              {!isLoading &&
                uniquesApis.slice(0,10).map((api) => (
                  <Box
                    key={api.latestVersion.id}
                    bgcolor="#444"
                    marginBottom={1}
                    padding={1}
                    onClick={()=>{router.push(`/apis/${api.latestVersion.id}/overview?version=${api.latestVersion.version}`)}}
                  >
                    <Typography variant="body1">
                      {api.latestVersion.id}({api.latestVersion.version})
                    </Typography>
                  </Box>
                ))}
            </Stack>
            {/* <Typography variant="h6" fontWeight="bold">
              APIs
            </Typography>
            <Divider sx={{ my: 2, backgroundColor: '#444' }} />
            <Box>
              <Typography>api1</Typography>
              <Typography variant="caption" color="gray">
                1.0
              </Typography>
            </Box>
            <Divider sx={{ my: 1, backgroundColor: '#444' }} />
            <Box>
              <Typography>hjl</Typography>
              <Typography variant="caption" color="gray">
                0.8
              </Typography>
            </Box> */}
          </Paper>
        </Grid2>

        {/* Components Section */}
        <Grid2 size={8}>
          <Paper
            elevation={3}
            sx={{
              padding: 4,
              height: "calc(100vh - 120px)",
              borderRadius: 2,
              display: "flex",
              justifyContent: "center",
              alignItems: "center",
              flexDirection: "column",
              color: "white",
              position: "relative",
            }}
          >
            {/* View All Button */}
            <Button
              variant="text"
              sx={{
                position: "absolute",
                top: 8,
                right: 8,
                fontSize: "0.8rem",
                textTransform: "none",
                border: "0.1px solid #555",
              }}
              className="text-[#888] dark:text-gray-400"
            >
              View All
            </Button>
            <Stack marginTop={6}>
            {!componentsLoading  && components.map((component: Component) => (
                <Box key={component?.versionedComponentId.componentId} bgcolor="#444" marginBottom={1} padding={1}>
                  <Typography variant="body1">{component.componentName}</Typography>
                </Box>
              ))}
              </Stack>

            {!componentsLoading && components.length ==0 &&<Box
              textAlign="center"
              sx={{
                borderRadius: 2,
                border: "2px dashed #444",
                padding: "5rem",
              }}
            >
              <Typography
                variant="h5"
                fontWeight="bold"
                className="text-[#888] dark:text-gray-400"
              >
                No Project Components
              </Typography>
              <Typography variant="body2" color="gray" mb={2}>
                Create your first component to get started
              </Typography>
              <Button
                variant="contained"
                startIcon={<AddIcon />}
                sx={{
                  backgroundColor: "#444",
                  color: "white",
                  "&:hover": { backgroundColor: "#555" },
                }}
              >
                Create New
              </Button>
            </Box>}
          </Paper>
        </Grid2>
      </Grid2>
      {/* Modal for Creating New API/Component */}
      <Modal open={!!open} onClose={handleClose}>
        <>
          {open === "api" && <CreateAPI onCreation={handleClose} />}
          {open === "component" && (
            <CreateComponentForm />
          )}
        </>
      </Modal>
      {/* Footer */}
      <OverviewFooter />
    </Box>
  );
};

export default ProjectDashboard;

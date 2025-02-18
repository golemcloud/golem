import React from "react";
import { ApiDefinition, Component as GolemComponent } from "@lib/types/api";
import NoComponentFound from "@components/components/no-component-found";
import { resources } from "@lib/resources";
import {Box, Divider, Grid2 as Grid, Paper, Stack, Typography} from "@mui/material";
import { useMemo,useState } from "react";
import ErrorBoundary from "@components/ui/error-boundary";
import FooterLinks from "@components/FooterLinks";
import CreateAPI from "@components/apis/create-api";
import ComponentCard from "@components/components/component-card";
import { Button2  } from "@components/ui/button";
import CustomModal from "@components/ui/custom/custom-modal";
import { calculateHoursDifference,calculateSizeInMB } from "@lib/utils";
import CreateComponentForm from "@components/components/new-component";
import useApiDefinitions from "@lib/hooks/use-api-definitons";
import useComponents from "@lib/hooks/use-component";
import { useNavigate } from "react-router-dom";

const ProjectDashboard = () => {
  const navigate = useNavigate();
  const [open, setOpen] = useState<string | null>(null);
  const { apiDefinitions, isLoading, error } = useApiDefinitions();
  const {components, isLoading: componentsLoading, error: componentError} = useComponents();
  
  //TODO we need limit the api we are showing in the Ui. for now we are showing all.
  const apiMap = apiDefinitions?.reduce<Record<string, ApiDefinition>>(
    (obj, api: ApiDefinition) => {
      obj[api.id] = api;
      return obj;
    },
    {}
  );

  const finalComponents = useMemo(() => {
    return Object.values(
      components?.reduce<Record<string, GolemComponent>>((obj, component) => {
        obj[component.versionedComponentId.componentId] = component;
        return obj;
      }, {}) || {}
    )?.reverse();
  }, [components]);

  const uniquesApis = Object.values(apiMap)?.reverse();

  function handleComponentClick(id: string, type: string) {
    type=="Ephemeral"?navigate(`/components/${id}/ephemeraloverview`):navigate(`/components/${id}/durableoverview`);
  }

  // const handleOpen = (type: string) => setOpen(type);
  const handleClose = () => setOpen(null);

  return (
    <main className='mx-auto max-w-7xl px-6 lg:px-8 min-h-[calc(100svh-84px)] py-4 flex h-full w-full flex-1 flex-col'>
      <Box className='mx-auto max-w-2xl lg:max-w-none gap-6 flex h-full w-full flex-1 flex-col'>
        {error === componentError && <ErrorBoundary message={error} />}

        <Grid container spacing={3} flexWrap='wrap'>
          <Grid size={{ xs: 12, md: 12, lg: 4 }}>
            <Paper
              elevation={3}
              className='p-2 min-h-auto lg:min-h-[calc(100vh-200px)] rounded-md flex flex-col border'
            >
              <Box className='flex justify-between'>
                <Typography variant='h5'>APIs</Typography>
                {uniquesApis?.length > 0 && (
                  <Button2
                    variant='primary'
                    size='sm'
                    className='text-muted-foreground'
                    onClick={() => navigate("/apis")}
                  >
                    View All
                  </Button2>
                )}
              </Box>
              {error !== componentError && <ErrorBoundary message={error} />}
              {uniquesApis?.length === 0 && (
                <NoComponentFound
                  heading='No APIs Available'
                  subheading='Create your first api to get started'
                  onClick={() => setOpen("api")}
                />
              )}
              {!error && !isLoading && uniquesApis?.length > 0 && (
                <Stack marginTop={2} sx={{ flex: 1, overflow: "hidden" }}>
                  {!isLoading &&
                    uniquesApis.slice(0, 8).map((api) => (
                      <React.Fragment key={api.id}>
                        <Divider className="border" />
                        <Box
                          key={api.id}
                          padding={3}
                          className='hover:bg-silver cursor-pointer'
                          onClick={() =>
                            navigate(
                              `/apis/${api.id}/overview?version=${api.version}`
                            )
                          }
                        >
                          <Box display='flex' justifyContent='space-between'>
                            <Typography
                              variant='body1'
                              className='overflow-hidden text-ellipsis whitespace-nowrap font-medium'
                            >
                              {api.id}
                            </Typography>
                            <Typography
                              variant='body2'
                              className='px-1 border border-gray-500 rounded'
                            >
                              {api.version}
                            </Typography>
                          </Box>
                        </Box>
                      </React.Fragment>
                    ))}
                </Stack>
              )}
            </Paper>
          </Grid>

          {/* Components Section */}
          <Grid size={{ xs: 12, md: 12, lg: 8 }}>
            <Paper
              elevation={3}
              className='p-2 min-h-auto lg:min-h-[calc(100vh-200px)] rounded-md flex flex-col'
            >
              <Box className='flex justify-between'>
                <Typography variant='h5'>Components</Typography>
                {components.length > 0 && (
                  <Button2
                    variant='primary'
                    size='sm'
                    className='text-muted-foreground'
                    onClick={() => navigate("/components")}
                  >
                    View All
                  </Button2>
                )}
              </Box>
              {error !== componentError && (
                <ErrorBoundary message={componentError} />
              )}
              {!componentError && !componentsLoading && (
                <Box className='grid w-full grid-cols-1 gap-3 lg:grid-cols-2 mt-2'>
                  {!componentError &&
                    finalComponents
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
                          onClick={() =>
                            handleComponentClick(
                              component.versionedComponentId.componentId!, component.componentType
                            )
                          }
                        />
                      ))}
                </Box>
              )}
              {finalComponents.length === 0 && (
                <NoComponentFound
                  heading='No Project Components'
                  subheading='Create your first component to get started'
                  onClick={() => setOpen("component")}
                />
              )}
            </Paper>
          </Grid>
        </Grid>

        <CustomModal
          open={!!open}
          onClose={handleClose}
          heading='Create a new Component'
        >
          {open === "api" && <CreateAPI onCreation={handleClose} />}
          {open === "component" && (
            <CreateComponentForm mode='create' onSubmitSuccess={handleClose} />
          )}
        </CustomModal>

        <FooterLinks variant='others' resources={resources} />
      </Box>
    </main>
  );
};

export default ProjectDashboard;

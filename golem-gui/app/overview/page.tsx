"use client";
import * as Imports from "@/components/imports";
import { ApiDefinition, Component as GolemComponent } from "@/types/api";
import Empty from "./empty";
import { resources } from "./utils";

const {
  React,
  Box,
  useMemo,
  useState,
  Typography,
  Paper,
  Grid,
  Stack,
  Divider,
  FooterLinks,
  CreateAPI,
  CreateComponentForm,
  useRouter,
  useApiDefinitions,
  useComponents,
  CustomModal,
  ComponentCard,
  calculateHoursDifference,
  calculateSizeInMB,
  Button2,
  ErrorBoundary,
} = Imports;

const ProjectDashboard = () => {
  const router = useRouter();
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

  function handleComponentClick(id: string) {
    console.log("Component Clicked");
    router.push(`/components/${id}/overview`);
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
                    onClick={() => router.push("/apis")}
                  >
                    View All
                  </Button2>
                )}
              </Box>
              {error !== componentError && <ErrorBoundary message={error} />}
              {uniquesApis?.length === 0 && (
                <Empty
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
                        <Divider sx={{ bgcolor: "#555" }} />
                        <Box
                          key={api.id}
                          padding={3}
                          className='hover:bg-[#444] cursor-pointer'
                          onClick={() =>
                            router.push(
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
              className='p-2 min-h-auto lg:min-h-[calc(100vh-200px)] rounded-md flex flex-col border'
            >
              <Box className='flex justify-between'>
                <Typography variant='h5'>Components</Typography>
                {components.length > 0 && (
                  <Button2
                    variant='primary'
                    size='sm'
                    className='text-muted-foreground'
                    onClick={() => router.push("/components")}
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
                              component.versionedComponentId.componentId!
                            )
                          }
                        />
                      ))}
                </Box>
              )}
              {finalComponents.length === 0 && (
                <Empty
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

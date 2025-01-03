'use client'
import React, { useState } from "react";
import {
  Alert,
  Box,
  CircularProgress,
  Container,
  Grid2 as Grid,
  IconButton,
  Link,
  Paper,
  Tooltip,
  Typography,
} from "@mui/material";
import { Button2 as Button } from "@/components/ui/button";
import {
  Add as AddIcon,
  Delete as DeleteIcon,
  Extension as PluginIcon,
  Link as LinkIcon,
  Tag as TagIcon,
  Widgets as WidgetsIcon,
} from "@mui/icons-material";
import usePlugins, { useDeletePlugin } from "@/lib/hooks/use-plugin";

import CreatePluginForm from "../../components/create-plugin";
import CustomModal from "@/components/CustomModal";
import { Plugin } from "@/types/api";
import ErrorBoundary from "@/components/erro-boundary";

export const PluginsPage = () => {
  const [open, setOpen] = useState(false);
  const { plugins, isLoading, error } = usePlugins();
  const { deletePlugin } = useDeletePlugin();

  const handleClose = () => setOpen(false);

  if (isLoading) {
    return (
      <Box
        display="flex"
        justifyContent="center"
        alignItems="center"
        height="64vh"
      >
        <CircularProgress />
        <Typography variant="body1" color="textSecondary" marginLeft={2}>
          Loading plugins...
        </Typography>
      </Box>
    );
  }

  if (error){
    return <ErrorBoundary message={error}/>
  }
  

  return (
    <main className="mx-auto max-w-7xl px-6 lg:px-8">
      <Box className="mx-auto max-w-2xl lg:max-w-none flex flex-col gap-6 py-6">
    <Container maxWidth="lg" >
      <Box
        display="flex"
        justifyContent="space-between"
        alignItems="center"
        marginBottom={4}
      >
        <Box>
          <Typography
            variant="h4"
            component="h1"
            display="flex"
            alignItems="center"
            gap={2}
          >
            <PluginIcon  />
            Plugins
          </Typography>
          <Typography variant="subtitle1"  className="text-muted-foreground">
            Manage your system plugins and extensions
          </Typography>
        </Box>
        <Button
          variant="primary"
          size="md"
          startIcon={<AddIcon />}
          onClick={(e) => {
            e.preventDefault();
            setOpen(true);
          }}
        >
          Create Plugin
        </Button>
      </Box>

      <Grid container spacing={3}>
        {plugins?.map((plugin: Plugin) => (
          <Grid  size={{xs:12, md:6}} key={`${plugin.name}-${plugin.version}`}>
            <Paper elevation={3}   className="border rounded-md"  sx={{p:2,"&:hover": { cursor: "pointer", boxShadow: "0px 5px 10px 0px #666" }}}>
              <Box
                display="flex"
                justifyContent="space-between"
                alignItems="flex-start"
              >
                <Box>
                  <Typography
                    variant="h6"
                    display="flex"
                    alignItems="center"
                    gap={1}
                  >
                    {plugin.specs.type === "OplogProcessor" ? (
                      <WidgetsIcon />
                    ) : (
                      <LinkIcon  />
                    )}
                    <Link
                      href={`/plugins/${plugin.name}/${plugin.version}`}
                      style={{ textDecoration: "none", color: "inherit" }}
                    >
                      {plugin.name}
                    </Link>
                  </Typography>
                  <Typography variant="body2" >
                    <TagIcon fontSize="small" />
                    Version {plugin.version}
                  </Typography>
                </Box>
                <Tooltip title="Delete plugin">
                  <IconButton
                    onClick={() => deletePlugin(plugin.name, plugin.version)}
                  >
                    <DeleteIcon color="error" />
                  </IconButton>
                </Tooltip>
              </Box>
              <Box marginTop={2}>
                <Typography variant="body1">{plugin.description}</Typography>

                <Box
                  display="flex"
                  justifyContent="space-between"
                  marginTop={2}
                >
                  <Typography variant="body2" className="text-muted-foreground" >
                    Type: {plugin.specs.type}
                  </Typography>
                  <Typography variant="body2" className="text-muted-foreground">
                    Scope: {plugin.scope.type}
                  </Typography>
                </Box>

                {plugin.specs.type === "OplogProcessor" && (
                  <Box
                    marginTop={2}
                    padding={2}
                  
                    borderRadius={1}
                  >
                    <Typography variant="body2" className="text-muted-foreground">
                      Component ID: {plugin.specs.componentId}
                    </Typography>
                    <Typography variant="body2" className="text-muted-foreground">
                      Version: {plugin.specs.componentVersion}
                    </Typography>
                  </Box>
                )}

                {plugin.specs.type === "ComponentTransformer" && (
                  <Box marginTop={2}>
                    <Typography
                      variant="body2"
                      component="a"
                      href={plugin.specs.validateUrl}
                      target="_blank"
                      rel="noopener noreferrer"
                    >
                      Validate URL: {plugin.specs.validateUrl}
                    </Typography>
                    <Typography
                      variant="body2"
                      component="a"
                      href={plugin.specs.transformUrl}
                      target="_blank"
                      rel="noopener noreferrer"
                    >
                      Transform URL: {plugin.specs.transformUrl}
                    </Typography>
                  </Box>
                )}
              </Box>
            </Paper>
          </Grid>
        ))}

        {(!plugins || plugins.length === 0) && (
          <Grid size={{xs:12}} >
            <Box className="dark:bg-[#1a2242] bg-[#f0f5ff] p-4 text-center border border-[#c6d3fa] dark:border-[#25366e] rounded-md"
            >
              <PluginIcon fontSize="large" />
              <Typography variant="h6" >
                No plugins found
              </Typography>
              <Typography variant="body2" >
                Create your first plugin to get started
              </Typography>
           
            </Box>
          </Grid>
        )}
      </Grid>
      <CustomModal open={open} onClose={handleClose} heading={"Create Plugin"}>
        <CreatePluginForm />
      </CustomModal>
    </Container>
    </Box>
    </main>
  );
};


export default PluginsPage;

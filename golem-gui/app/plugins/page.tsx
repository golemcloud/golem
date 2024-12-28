'use client'
import React, { useState } from "react";
import {
  Alert,
  Box,
  Button,
  CircularProgress,
  Container,
  Grid,
  IconButton,
  Link,
  Paper,
  Tooltip,
  Typography,
} from "@mui/material";
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

  if (error)
    return (
      <Box
        display="flex"
        justifyContent="center"
        alignItems="center"
        height="100vh"
      >
        <Alert severity="error">Error: {error}</Alert>
      </Box>
    );

  return (
    <Container maxWidth="lg">
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
            <PluginIcon color="primary" />
            Plugins
          </Typography>
          <Typography variant="subtitle1" color="textSecondary">
            Manage your system plugins and extensions
          </Typography>
        </Box>
        <Button
          variant="contained"
          color="primary"
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
          <Grid item xs={12} md={6} key={`${plugin.name}-${plugin.version}`}>
            <Paper elevation={3} sx={{ p: 2 }}>
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
                      <WidgetsIcon color="secondary" />
                    ) : (
                      <LinkIcon color="success" />
                    )}
                    <Link
                      href={`/plugins/${plugin.name}/${plugin.version}`}
                      style={{ textDecoration: "none", color: "inherit" }}
                    >
                      {plugin.name}
                    </Link>
                  </Typography>
                  <Typography variant="body2" color="textSecondary">
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
                  <Typography variant="body2" color="textSecondary">
                    Type: {plugin.specs.type}
                  </Typography>
                  <Typography variant="body2" color="textSecondary">
                    Scope: {plugin.scope.type}
                  </Typography>
                </Box>

                {plugin.specs.type === "OplogProcessor" && (
                  <Box
                    marginTop={2}
                    padding={2}
                    bgcolor="grey.100"
                    borderRadius={1}
                  >
                    <Typography variant="body2" color="textSecondary">
                      Component ID: {plugin.specs.componentId}
                    </Typography>
                    <Typography variant="body2" color="textSecondary">
                      Version: {plugin.specs.componentVersion}
                    </Typography>
                  </Box>
                )}

                {plugin.specs.type === "ComponentTransformer" && (
                  <Box marginTop={2}>
                    <Typography
                      variant="body2"
                      color="primary"
                      component="a"
                      href={plugin.specs.validateUrl}
                      target="_blank"
                      rel="noopener noreferrer"
                    >
                      Validate URL: {plugin.specs.validateUrl}
                    </Typography>
                    <Typography
                      variant="body2"
                      color="primary"
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
          <Grid item xs={12}>
            <Paper elevation={3} sx={{ p: 4, textAlign: "center" }}>
              <PluginIcon fontSize="large" color="disabled" />
              <Typography variant="h6" color="textSecondary">
                No plugins found
              </Typography>
              <Typography variant="body2" color="textSecondary">
                Create your first plugin to get started
              </Typography>
            </Paper>
          </Grid>
        )}
      </Grid>
      <CustomModal open={open} onClose={handleClose} heading={"Create Plugin"}>
        <CreatePluginForm />
      </CustomModal>
    </Container>
  );
};


export default PluginsPage;

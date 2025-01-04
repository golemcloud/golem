"use client";
import React, { useCallback, useEffect, useMemo, useState } from "react";
import {
  Box,
  CircularProgress,
  Container,
  Grid2 as Grid,
  IconButton,
  InputAdornment,
  Link,
  Pagination,
  Paper,
  TextField,
  Tooltip,
  Typography,
} from "@mui/material";
import { Button2 as Button, Button2 } from "@/components/ui/button";
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
import NotFoundCard from "@/components/not-found-card";
import SearchIcon from "@mui/icons-material/Search";

export const PluginsPage = () => {
  const [open, setOpen] = useState(false);
  const { plugins, isLoading, error } = usePlugins();
  const { deletePlugin } = useDeletePlugin();
  const [searchQuery, setSearchQuery] = useState("");
  const [currentPage, setCurrentPage] = useState(1);
  const handleClose = () => setOpen(false);

  const checkForMatch = useCallback(
    (plugin: Plugin) => {
      if (!searchQuery || searchQuery?.length <= 2) {
        return true;
      }
      return plugin.name.toLowerCase().includes(searchQuery.toLowerCase());
    },
    [searchQuery]
  );

  const finalPlugins = useMemo(() => {
    if (!plugins) return [];
    return plugins.filter(checkForMatch);
  }, [plugins, checkForMatch]);

  useEffect(() => {
    if (searchQuery && searchQuery?.length > 2) {
      setCurrentPage(1);
    }
  }, [finalPlugins]);
  const itemsPerPage = 10;

  const totalPages = Math.ceil(finalPlugins.length / itemsPerPage);
  const paginatedComponents = useMemo(() => {
    const startIndex = (currentPage - 1) * itemsPerPage;
    const endIndex = startIndex + itemsPerPage;
    return finalPlugins.slice(startIndex, endIndex);
  }, [finalPlugins, currentPage]);

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

  if (error) {
    return <ErrorBoundary message={error} />;
  }

  return (
    <main className="mx-auto max-w-7xl px-6 lg:px-8">
      <Box className="mx-auto max-w-2xl lg:max-w-none flex flex-col gap-6 py-6">
        <Box
          display="flex"
          justifyContent="space-between"
          alignItems="center"
          mb={2}
          gap={2}
        >
          <TextField
            placeholder="Search Plugins..."
            variant="outlined"
            size="small"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            InputProps={{
              startAdornment: (
                <InputAdornment position="start">
                  <SearchIcon sx={{ color: "grey.500" }} />
                </InputAdornment>
              ),
            }}
            className="flex-1"
          />
          <Button2
            variant="default"
            endIcon={<AddIcon />}
            size="md"
            onClick={(e) => {
              e.preventDefault();
              setOpen(true);
            }}
          >
            New
          </Button2>
        </Box>
        <Grid container spacing={3}>
          {paginatedComponents?.map((plugin: Plugin) => (
            <Grid
              size={{ xs: 12, md: 6 }}
              key={`${plugin.name}-${plugin.version}`}
            >
              <Paper
                elevation={3}
                className="border rounded-md"
                sx={{
                  p: 2,
                  "&:hover": {
                    cursor: "pointer",
                    boxShadow: "0px 5px 10px 0px #666",
                  },
                }}
              >
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
                        <LinkIcon />
                      )}
                      <Link
                        href={`/plugins/${plugin.name}/${plugin.version}`}
                        style={{ textDecoration: "none", color: "inherit" }}
                      >
                        {plugin.name}
                      </Link>
                    </Typography>
                    <Typography variant="body2">
                      <TagIcon fontSize="small" />
                      Version {plugin.version}
                    </Typography>
                  </Box>
                  <Tooltip title="Delete plugin">
                    <Button2
                      variant="error"
                      size="sm"
                      onClick={() => deletePlugin(plugin.name, plugin.version)}
                    >
                      <DeleteIcon color="error" />
                    </Button2>
                  </Tooltip>
                </Box>
                <Box marginTop={2}>
                  <Typography variant="body1">{plugin.description}</Typography>

                  <Box
                    display="flex"
                    justifyContent="space-between"
                    marginTop={2}
                  >
                    <Typography
                      variant="body2"
                      className="text-muted-foreground"
                    >
                      Type: {plugin.specs.type}
                    </Typography>
                    <Typography
                      variant="body2"
                      className="text-muted-foreground"
                    >
                      Scope: {plugin.scope.type}
                    </Typography>
                  </Box>

                  {plugin.specs.type === "OplogProcessor" && (
                    <Box marginTop={2} borderRadius={1}>
                      <Typography
                        variant="body2"
                        className="text-muted-foreground"
                      >
                        Component ID: {plugin.specs.componentId}
                      </Typography>
                      <Typography
                        variant="body2"
                        className="text-muted-foreground"
                      >
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
            <Grid size={{ sm: 12 }}>
              <NotFoundCard
                heading="No plugins found"
                subheading="Create your first plugin to get started"
                icon={<PluginIcon fontSize="large" />}
              />
            </Grid>
          )}
        </Grid>
        <Box mt={4} display="flex" justifyContent="center">
          <Pagination
            count={totalPages}
            page={currentPage}
            onChange={(_, value) => setCurrentPage(value)}
            color="primary"
            className="pagination"
          />
        </Box>
        <CustomModal
          open={open}
          onClose={handleClose}
          heading={"Create New Plugin"}
        >
          <CreatePluginForm />
        </CustomModal>
      </Box>
    </main>
  );
};

export default PluginsPage;

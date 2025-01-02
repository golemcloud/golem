"use client";

import React, { useCallback, useMemo, useState } from "react";
import {
  Alert,
  Box,
  InputAdornment,
  TextField,
  Typography,
} from "@mui/material";
import SearchIcon from "@mui/icons-material/Search";
import AddIcon from "@mui/icons-material/Add";
import CreateAPI from "@/components/create-api";
import ApiIcon from "@mui/icons-material/Api";
import { useRouter } from "next/navigation";
import { ApiDefinition } from "@/types/api";
import CustomModal from "@/components/CustomModal";
import useApiDefinitions from "@/lib/hooks/use-api-definitons";
import ApiInfoCard from "@/components/api-info-card";
import { Button2 } from "@/components/ui/button";

const ComponentsPage = () => {
  const [open, setOpen] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");
  const { apiDefinitions, isLoading, error } = useApiDefinitions();
  const router = useRouter();

  const handleOpen = () => setOpen(true);
  const handleClose = () => setOpen(false);

  const handleApiClick = (apiId: string) => {
    router.push(`/apis/${apiId}/overview`);
  };

  const finalApis = useMemo(() => {
    return Object.values(
      apiDefinitions?.reduce<Record<string, ApiDefinition>>(
        (obj, api: ApiDefinition) => {
          obj[api.id] = api;
          return obj;
        },
        {}
      ) || {}
    ).sort(
      (a, b) =>
        new Date(b.createdAt).getTime() - new Date(a.createdAt).getTime()
    );
  }, [apiDefinitions]);
  const checkForMatch = useCallback(
    (api: ApiDefinition) => {
      if (!searchQuery || searchQuery?.length <= 2) {
        return true;
      }

      return api.id.toLowerCase().includes(searchQuery.toLowerCase());
    },
    [searchQuery]
  );

  return (
    <main className="mx-auto max-w-7xl px-6 lg:px-8 min-h-[calc(100svh-84px)] py-4 flex h-full w-full flex-1 flex-col">
      <Box className="mx-auto max-w-2xl lg:max-w-none gap-6 flex h-full w-full flex-1 flex-col">
        {error && (
          <Box sx={{ display: "flex", justifyContent: "center" }}>
            {error && <Alert severity="error">{error}</Alert>}
          </Box>
        )}
        {!error && !isLoading && (
          <>
            <Box
              display="flex"
              justifyContent="space-between"
              alignItems="center"
              mb={1}
              gap={2}
            >
              <TextField
                placeholder="Search APIs..."
                variant="outlined"
                size="small"
                value={searchQuery}
                onChange={(e) => setSearchQuery(e.target.value)} // Update search query
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
                onClick={handleOpen}
              >
                New
              </Button2>
            </Box>

            {finalApis.length === 0 ? (
              <Box
                sx={{
                  color: "#aaa",
                  textAlign: "center",
                  py: 8,
                  border: "2px dashed #333",
                  borderRadius: 2,
                }}
              >
                <Box display="flex" justifyContent="center" mb={2}>
                  <Box
                    component="span"
                    sx={{
                      fontSize: 50,
                      color: "#666",
                    }}
                  >
                    <ApiIcon sx={{ fontSize: 40 }} />
                  </Box>
                </Box>
                <Typography
                  variant="h6"
                  fontWeight="bold"
                  gutterBottom
                  className="text-[#888] dark:text-gray-400"
                >
                  No APIs Components
                </Typography>
                <Typography variant="body2" color="grey.500">
                  No APIs found matching your search.
                </Typography>
              </Box>
            ) : (
              <Box className="grid w-full grid-cols-1 gap-3 lg:grid-cols-2 xl:grid-cols-3  mt-2">
                {!isLoading &&
                  finalApis.map((api: ApiDefinition) =>
                    checkForMatch(api) ? (
                      <ApiInfoCard
                        key={api.id}
                        name={api.id}
                        version={api.version}
                        routesCount={api.routes.length}
                        locked={api.draft}
                        onClick={() => handleApiClick(api.id)}
                      />
                    ) : null
                  )}
              </Box>
            )}

            <CustomModal
              open={open}
              onClose={handleClose}
              heading="Create New API"
            >
              <CreateAPI onCreation={handleClose} />
            </CustomModal>
          </>
        )}
      </Box>
    </main>
  );
};

export default ComponentsPage;

"use client";

import React, { useState } from "react";
import {
  Box,
  Button,
  Container,
  InputAdornment,
  TextField,
  Typography,
  Modal,
  Card,
  CardContent,
} from "@mui/material";
import SearchIcon from "@mui/icons-material/Search";
import AddIcon from "@mui/icons-material/Add";
import CreateAPI from "@/components/create-api";
import ApiIcon from "@mui/icons-material/Api";
import { useRouter } from "next/navigation";
import useSWR from "swr";
import { fetcher } from "@/lib/utils";
import { ApiDefinition } from "@/types/api";
import CustomModal from "@/components/CustomModal";

const ComponentsPage = () => {
  const [open, setOpen] = useState(false);
  //move this custom hook and us it here.
  const { data: apiData, isLoading } = useSWR("?path=api/definitions", fetcher);
  const apis = (apiData?.data || []) as ApiDefinition[];
  const router = useRouter();
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

  const handleOpen = () => setOpen(true);
  const handleClose = () => setOpen(false);

  const handleApiClick = (apiId: string, version:string) => {
    // Navigate to the API details page within the project
    router.push(`/apis/${apiId}/overview?version=${version}`);
  };

  return (
    <Container maxWidth="lg" sx={{ mt: 5, height: "100vh" }}>
      <Box
        display="flex"
        justifyContent="space-between"
        alignItems="center"
        mb={3}
        gap={2}
      >
        <TextField
          placeholder="Search APIs..."
          variant="outlined"
          size="small"
          InputProps={{
            startAdornment: (
              <InputAdornment position="start">
                <SearchIcon sx={{ color: "grey.500" }} />
              </InputAdornment>
            ),
          }}
          className="flex-1"
        />
        <Button
          variant="contained"
          startIcon={<AddIcon />}
          sx={{
            textTransform: "none",
            marginLeft: "2px",
          }}
          onClick={handleOpen}
        >
          New
        </Button>
      </Box>

      {apis.length === 0 ? (
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
            Create a new API to get started.
          </Typography>
        </Box>
      ) : (
        <Box sx={{ display: "flex", flexWrap: "wrap", gap: 2 }}>
          {!isLoading && Object.values(apiMap)?.map((api) => (
            <Card
              key={api.latestVersion.id}
              sx={{
                cursor: "pointer",
                width: "200px",
                "&:hover": { boxShadow: 4 },
                transition: "all 0.3s ease",
              }}
              onClick={() => handleApiClick(api.latestVersion.id!, api.latestVersion.version!)}
            >
              <CardContent>
                <Typography variant="h6">{api.latestVersion.id}</Typography>
              </CardContent>
            </Card>
          ))}
        </Box>
      )}

      <CustomModal open={open} onClose={handleClose} heading="Create New API">
        <CreateAPI onCreation={handleClose}/>
      </CustomModal>
    </Container>
  );
};

export default ComponentsPage;

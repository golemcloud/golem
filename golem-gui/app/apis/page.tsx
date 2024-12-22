"use client";

import React, { useState } from "react";
import {
  Box,
  Button,
  Container,
  InputAdornment,
  TextField,
  Typography,
  Card,
  CardContent,
} from "@mui/material";
import SearchIcon from "@mui/icons-material/Search";
import AddIcon from "@mui/icons-material/Add";
import CreateAPI from "@/components/create-api";
import ApiIcon from "@mui/icons-material/Api";
import { useRouter } from "next/navigation";
import { ApiDefinition } from "@/types/api";
import CustomModal from "@/components/CustomModal";
import useApiDefinitions from "@/lib/hooks/use-api-definitons";

const ComponentsPage = () => {
  const [open, setOpen] = useState(false);
  //Ideally we are not sure about latest version. as we are getting the every version separately. there is no way of knowing what is the latest version.
  //out of all the fetched api's considering the last one as latest. there is chance that if pagination applied on the api's latest version may show wrong.
  const { apiDefinitions, getApiDefintion, isLoading } = useApiDefinitions();
  const router = useRouter();
  const apiMap = apiDefinitions?.reduce<Record<string, ApiDefinition | null>>(
    (obj, api: ApiDefinition) => {
      if (api.id in obj) {
        return obj;
      } else {
        obj[api.id] = getApiDefintion(api.id)?.data || null;
      }
      return obj;
    },
    {}
  );

  const handleOpen = () => setOpen(true);
  const handleClose = () => setOpen(false);

  const handleApiClick = (apiId: string) => {
    // Navigate to the API details page within the project
    router.push(`/apis/${apiId}/overview`);
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

      {apiDefinitions.length === 0 ? (
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
          {!isLoading &&
            Object.values(apiMap)?.map((api: ApiDefinition | null) =>
              api ? (
                <Card
                  key={api.id}
                  sx={{
                    cursor: "pointer",
                    width: "200px",
                    "&:hover": { boxShadow: 4 },
                    transition: "all 0.3s ease",
                  }}
                  onClick={() => handleApiClick(api.id!)}
                >
                  <CardContent>
                    <Typography variant="h6">{api.id}</Typography>
                  </CardContent>
                </Card>
              ) : null
            )}
        </Box>
      )}
      <CustomModal open={open} onClose={handleClose} heading="Create New API">
        <CreateAPI onCreation={handleClose} />
      </CustomModal>
    </Container>
  );
};

export default ComponentsPage;

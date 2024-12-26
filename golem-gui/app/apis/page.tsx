"use client";

import React, { useState } from "react";
import {
  Alert,
  Box,
  Button,
  Container,
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

  // Filter APIs based on search query
  const filteredApis = apiDefinitions?.filter((api: ApiDefinition) =>
    api.id.toLowerCase().includes(searchQuery.toLowerCase())
  );

  return (
    <Container maxWidth="lg" sx={{ mt: 5, height: "100vh" }}>
       {error && (
        <Box sx={{ display: "flex", justifyContent: "center" }}>
          {error && <Alert severity="error">{error}</Alert>}
        </Box>
      )}
      {!error && !isLoading &&<>           
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

      {filteredApis.length === 0 ? (
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
        <Box sx={{ display: "flex", flexWrap: "wrap", gap: 2 }}>
          {!isLoading &&
            filteredApis.map((api: ApiDefinition) => (
              <ApiInfoCard
                key={api.id}
                name={api.id}
                version={api.version}
                routesCount={api.routes.length}
                locked={api.draft}
                onClick={() => handleApiClick(api.id)}
              />
            ))}
        </Box>
      )}

      <CustomModal open={open} onClose={handleClose} heading="Create New API">
        <CreateAPI onCreation={handleClose} />
      </CustomModal>
      </>}
    </Container>
  );
};

export default ComponentsPage;

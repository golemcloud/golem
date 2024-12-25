"use client";
import React, { useState } from "react";
import {
  Box,
  Button,
  Container,
  InputAdornment,
  TextField,
  Typography,
  IconButton,
} from "@mui/material";
import SearchIcon from "@mui/icons-material/Search";
import AddIcon from "@mui/icons-material/Add";
import GridViewIcon from "@mui/icons-material/GridView";
import ListIcon from "@mui/icons-material/List";
import CreateComponentForm from "@/components/new-component";
import WidgetsIcon from "@mui/icons-material/Widgets";
import { Component } from "@/types/api";
import { useRouter } from "next/navigation";
import useComponents from "@/lib/hooks/use-component";
import CustomModal from "@/components/CustomModal";
import ComponentCard from "@/components/components-card";
import { calculateHoursDifference, calculateSizeInMB } from "@/lib/utils";
import clsx from "clsx";
import ComponentTable from "@/components/ui/generic-table";



const ComponentsPage = () => {
  const [open, setOpen] = useState(false);
  const [activeButton, setActiveButton] = useState("grid");
  const handleOpen = () => setOpen(true);
  const handleClose = () => setOpen(false);
  const [viewMode, setViewMode] = useState("card");
  const [searchQuery, setSearchQuery] = useState("");

  const handleActiveButton = (button: string) => {
    setActiveButton(button);
    setViewMode(button === "grid" ? "card" : "table");
  };
  const router = useRouter();
  const { components, isLoading } = useComponents();

  function handleComponentClick(id: string) {
    console.log("Component Clicked");
    router.push(`/components/${id}/overview`);
  }

  // Filter APIs based on search query
  const filteredComponents = components?.filter((component: Component) =>
    component.componentName.toLowerCase().includes(searchQuery.toLowerCase())
  );

  return (
    <Container maxWidth="lg" sx={{ mt: 5}}>
      {/* Search Bar and Buttons */}
      <Box
        display="flex"
        justifyContent="space-between"
        alignItems="center"
        mb={3}
        gap={2}
      >
        {/* Search Field */}
        <TextField
          placeholder="Search Components..."
          variant="outlined"
          value={searchQuery}
          size="small"
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

        {/* Buttons */}
        <Box className="flex gap-0 rounded-md dark:bg-[#333] bg-gray-200 p-1">
          <IconButton
            onClick={() => handleActiveButton("grid")}
            className={clsx(
              "p-2 rounded-md transition-colors",
              activeButton === "grid"
                ? "dark:bg-black  bg-gray-500 text-white hover:bg-gray-500"
                : "dark:text-gray-200 text-gray-700"
            )}
          >
            <GridViewIcon />
          </IconButton>
          <IconButton
            onClick={() => handleActiveButton("list")}
            className={clsx(
              "p-2 rounded-md ",
              activeButton === "list"
                ? "dark:bg-black bg-gray-500 text-white  hover:bg-gray-500"
                : "dark:text-gray-200 text-gray-700"
            )}
          >
            <ListIcon />
          </IconButton>
        </Box>
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

      {filteredComponents.length > 0 ? (
        viewMode === "card" ? (
          <Box sx={{ display: "flex", flexWrap: "wrap", gap: 2 }}>
            {!isLoading &&
              filteredComponents.map((item) => (
                <ComponentCard
                  key={item.versionedComponentId.componentId}
                  id={item.versionedComponentId.componentId}
                  title={item.componentName}
                  time={calculateHoursDifference(item.createdAt)}
                  version={item.versionedComponentId.version}
                  exports={item.metadata.exports.length}
                  size={calculateSizeInMB(item.componentSize)}
                  componentType={item.componentType}
                  onClick={() =>
                    handleComponentClick(item.versionedComponentId.componentId)
                  }
                />
              ))}
          </Box>
        ) : (
          <ComponentTable<Component>
              data={filteredComponents}
              columns={[
                {
                  key: "componentName",
                  label: "Name",
                  accessor: (item) => item.componentName,
                },
                {
                  key: "componentType",
                  label: "Type",
                  accessor: (item) => item.componentType,
                },
                {
                  key: "componentSize",
                  label: "Size",
                  accessor: (item) => calculateSizeInMB(item.componentSize),
                },
                {
                  key: "metadata.exports",
                  label: "Exports",
                  accessor: (item) => item.metadata.exports.length,
                },
              ]}
              onRowClick={(item) => handleComponentClick(item.versionedComponentId.componentId)} />
        )
      ) : (
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
            <WidgetsIcon sx={{ fontSize: 40, color: "#666" }} />
          </Box>
          <Typography variant="h6" fontWeight="bold" gutterBottom>
            No Project Components
          </Typography>
          <Typography variant="body2" color="grey.500">
            Create a new component to get started.
          </Typography>
        </Box>
      )}
      {/* Modal for Creating New API/Component */}
      <CustomModal
        open={open}
        onClose={handleClose}
        heading="Create a new Component"
      >
        <CreateComponentForm
          onSubmitSuccess={() => handleClose()}
          mode="create"
        />
      </CustomModal>
      <br /><br /><br />
    </Container>
  );
};

export default ComponentsPage;

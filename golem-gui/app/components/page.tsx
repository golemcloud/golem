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
import { Card, CardContent } from "@mui/material";
import { Component } from "@/types/api";
import { useRouter } from "next/navigation";
import useComponents from "@/lib/hooks/use-component";
import CustomModal from "@/components/CustomModal";
import CustomCard from "@/components/ui/custom-card";
import ComponentCard from "@/components/components-card";

const ComponentsPage = () => {
  const [open, setOpen] = useState(false);

  const handleOpen = () => setOpen(true);
  const handleClose = () => setOpen(false);
  const router = useRouter();

  const {components, isLoading } = useComponents();

  // const components = (componentData?.data || []) as Component[];

  function handleComponentClick(id: string){
    console.log("Component Clicked")
    router.push(`/components/${id}`);
  }

  function calculateHoursDifference(createdAt: string): number {
    const createdAtDate = new Date(createdAt);
    const currentDate = new Date();
    const differenceInMs = currentDate.getTime() - createdAtDate.getTime();
    const differenceInHours = Math.round(differenceInMs / (1000 * 60 * 60));
  
    return differenceInHours;
  }
  
  function calculateSizeInMB(sizeInBytes: number): string {
    return (sizeInBytes / (1024 * 1024)).toFixed(2);;
  }

  return (
    <Container maxWidth="lg" sx={{ mt: 5, height: "100vh" ,overflow:"auto"}}>
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

        {/* Buttons */}
        <Box display="flex" gap={1}  sx={{borderRadius:'5px',bgColor:"#555"}}>
          <IconButton>
            <GridViewIcon />
          </IconButton>
          <IconButton>
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
  

      {components.length > 0 ? (
        <Box sx={{ display: "flex", flexWrap: "wrap", gap: 2 }}>
          {!isLoading && components?.map((item: Component) => (
          <ComponentCard
            id={item.versionedComponentId.componentId}
            title={item.componentName}
            time={calculateHoursDifference(item.createdAt)}
            version={item.versionedComponentId.version}
            exports={item.metadata.exports.length}
            size={calculateSizeInMB(item.componentSize)}
            componentType={item.componentType}
            onClick={() => handleComponentClick(item.versionedComponentId.componentId!)}
          />
            
          ))}
        </Box>)
      :(<Box
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
            <WidgetsIcon sx={{ fontSize: 40 }}/>
          </Box>
          <Typography
            variant="h6"
            fontWeight="bold"
            gutterBottom
            className="text-[#888] dark:text-gray-400"
          >
            No Project Components
          </Typography>
          <Typography variant="body2" color="grey.500">
            Create a new component to get started.
          </Typography>
        </Box>
      </Box>)}
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
    </Container>
  );
};

export default ComponentsPage;

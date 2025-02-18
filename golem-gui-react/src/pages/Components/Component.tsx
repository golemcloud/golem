"use client";
import  { useCallback, useEffect, useMemo, useState } from "react";
import {
  Box,
  InputAdornment,
  TextField,
  IconButton,
  Pagination,
} from "@mui/material";
import SearchIcon from "@mui/icons-material/Search";
import AddIcon from "@mui/icons-material/Add";
import GridViewIcon from "@mui/icons-material/GridView";
import ListIcon from "@mui/icons-material/List";
import CreateComponentForm from "@components/components/new-component";
import WidgetsIcon from "@mui/icons-material/Widgets";
import { Component } from "@lib/types/api"; 
import { useNavigate } from "react-router-dom";
import useComponents from "@lib/hooks/use-component";
import CustomModal from "@components/ui/custom/custom-modal";
import ComponentInfoCard from "@components/components/main-component-card"; 
import { calculateHoursDifference, calculateSizeInMB } from  "@lib/utils"
import { Button2 } from "@components/ui/button"; 
import clsx from "clsx";
import ComponentTable from "@components/ui/generic-table";
import ErrorBoundary from "@components/ui/error-boundary";
import NotFoundCard from "@components/ui/not-found-card";

const ComponentsPage = () => {
  const [open, setOpen] = useState(false);
  const [activeButton, setActiveButton] = useState("grid");
  const [viewMode, setViewMode] = useState("card");
  const [searchQuery, setSearchQuery] = useState("");
  const [currentPage, setCurrentPage] = useState(1);
  const itemsPerPage = 10;

  const handleOpen = () => setOpen(true);
  const handleClose = () => setOpen(false);
  const handleActiveButton = (button: string) => {
    setActiveButton(button);
    setViewMode(button === "grid" ? "card" : "table");
  };
  const navigate = useNavigate();
  const { components, isLoading, error } = useComponents();
  
  function handleComponentClick(id: string, type: string) {
    type=="Ephemeral"?navigate(`/components/${id}/ephemeraloverview`):navigate(`/components/${id}/durableoverview`);
  }

  const checkForMatch = useCallback(
    (component: Component) => {
      if (!searchQuery || searchQuery?.length <= 2) {
        return true;
      }
      return component.componentName
        .toLowerCase()
        .includes(searchQuery.toLowerCase());
    },
    [searchQuery]
  );

  const finalComponents = useMemo(() => {
    return Object.values(
      components?.reduce<Record<string, Component>>((obj, component) => {
        obj[component.versionedComponentId.componentId] = component;
        return obj;
      }, {}) || {}
    )?.reverse()
  }, [components])?.filter(checkForMatch);


  useEffect(()=>{
    if(searchQuery && searchQuery?.length>2){
      setCurrentPage(1)
    }
  }, [finalComponents])

  // Pagination Logic
  const totalPages = Math.ceil(finalComponents.length / itemsPerPage);
  const paginatedComponents = useMemo(() => {
    const startIndex = (currentPage - 1) * itemsPerPage;
    const endIndex = startIndex + itemsPerPage;
    return finalComponents.slice(startIndex, endIndex);
  }, [finalComponents, currentPage]);

  return (
    <main className="mx-auto max-w-7xl px-6 lg:px-8">
      <Box className="mx-auto max-w-2xl lg:max-w-none flex flex-col gap-6 py-6">
        {error && <ErrorBoundary message={error}/>}
        {!error && !isLoading && (
          <>
            {/* Search Bar and Buttons */}
            <Box
              display="flex"
              justifyContent="space-between"
              alignItems="center"
              mb={2}
              gap={2}
            >
              {/* Need to debounce logic to reduce the computation*/}
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

              <Box className="flex rounded-md dark:bg-[#333] bg-gray-200 p-1">
                <Box
                  onClick={() => handleActiveButton("grid")}
                  className={clsx(
                    "p-2 rounded-md transition-colors",
                    activeButton === "grid"
                      ? "dark:bg-black bg-gray-500 text-white hover:bg-gray-500"
                      : "dark:text-gray-200 text-gray-700"
                  )}
                >
                  <GridViewIcon />
                </Box>
                <Box
                  onClick={() => handleActiveButton("list")}
                  className={clsx(
                    "p-2 rounded-md",
                    activeButton === "list"
                      ? "dark:bg-black bg-gray-500 text-white hover:bg-gray-500"
                      : "dark:text-gray-200 text-gray-700"
                  )}
                >
                  <ListIcon />
                </Box>
              </Box>
              <Button2
                variant="primary"
                size="md"
                endIcon={<AddIcon />}
                onClick={handleOpen}
              >
                New
              </Button2>
            </Box>

            {paginatedComponents.length > 0 ? (
              viewMode === "card" ? (
                <Box className="grid w-full grid-cols-1 lg:grid-cols-2 gap-6 xl:grid-cols-2">
                  {!isLoading &&
                    paginatedComponents.map((item) => (
                      <ComponentInfoCard
                        key={item.versionedComponentId.componentId}
                        id={item.versionedComponentId.componentId}
                        title={item.componentName}
                        time={calculateHoursDifference(item.createdAt)}
                        version={item.versionedComponentId.version}
                        exports={item.metadata.exports.length}
                        size={calculateSizeInMB(item.componentSize)}
                        componentType={item.componentType}
                        onClick={() =>
                          handleComponentClick(
                            item.versionedComponentId.componentId,item.componentType
                          )
                        }
                      />
                    ))}
                </Box>
              ) : (
                <ComponentTable<Component>
                  data={paginatedComponents}
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
                  onRowClick={(item) =>
                    handleComponentClick(item.versionedComponentId.componentId,item.componentType)
                  }
                />
              )
            ) : (
               <NotFoundCard heading="No Components available" subheading="Create a  new component to get started" icon={<WidgetsIcon fontSize="large"/>}/>
            )}
            {/* Pagination Controls */}
            {/* TODO handle pagination dark theme and light theme */}
            <Box mt={4} display="flex" justifyContent="center">
              <Pagination
                count={totalPages}
                page={currentPage}
                onChange={(_, value) => setCurrentPage(value)}
                color="primary"
                className="pagination"
              />
            </Box>
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
          </>
        )}
      </Box>
    </main>
  );
};

export default ComponentsPage;


"use client";
import React, { useState } from "react";
import {
  Box,
  Button,
  Typography,
  TextField,
  Paper,
} from "@mui/material";
import { useForm, Controller } from "react-hook-form";

interface FormData {
  project: string;
  componentName: string;
  type: string;
}

interface CreateProjectFromProps {
  onCreation?: () => void;
}

export default function CreateProjectFrom({ onCreation }: CreateProjectFromProps) {
  const [files, setFiles] = useState<File[]>([]);
  const [wasmFile, setWasmFile] = useState<File | null>(null);

  const { handleSubmit, control } = useForm<FormData>({
    defaultValues: {
      project: "",
      componentName: "",
      type: "durable",
    },
  });


  const handleFileDelete = (index: number) => {
    const updatedFiles = [...files];
    updatedFiles.splice(index, 1);
    setFiles(updatedFiles);
  };

  const onSubmit = (data: FormData) => {
    console.log("Form Data:", data);
    console.log("WASM File:", wasmFile);
    console.log("Uploaded Files:", files);
    // onCreation();
  };

  return (
      <Paper
        elevation={3}
        sx={{
          maxWidth: "800px",
          width: "60%",
          mx: "auto",
          mt: 5,
          color: "#FFFFFF",
          borderRadius: "8px",
          height: "calc(100vh - 120px)",
          overflowY: "auto",
        }}
      >
        <Typography
          variant="h5"
          mb={2}
          sx={{
            position: "sticky",
            borderBottom: "0.1px solid #888",
            top: 0,
            zIndex: 1000,
            padding: "1rem",
          }}
          className="dark:bg-[#333] dark:text-[#fff] text-[#000] bg-white"
        >
          Create a new Project
        </Typography>

        <form onSubmit={handleSubmit(onSubmit)} className="p-6 px-10">
          <Typography variant="body2" color="gray" mb={3}>
            Components are the building blocks for your project
          </Typography>

          {/* Project and Component Name */}
          <Box display="flex" gap={2} mb={3}>
            <Controller
              name="project"
              control={control}
              render={({ field }) => (
                <TextField label="Project" variant="outlined" fullWidth {...field} />
              )}
            />
            <Controller
              name="componentName"
              control={control}
              render={({ field }) => (
                <TextField label="Component Name" variant="outlined" fullWidth {...field} />
              )}
            />
          </Box>

          <Box display="flex" flexDirection="column" gap={1} mb={3}>
            {files.map((file, index) => (
              <Box
                key={index}
                display="flex"
                justifyContent="space-between"
                alignItems="center"
                border="1px solid gray"
                p={2}
              >
                <Typography variant="body2">{file.name}</Typography>
                <Button
                  variant="outlined"
                  color="error"
                  onClick={() => handleFileDelete(index)}
                >
                  Delete
                </Button>
              </Box>
            ))}
          </Box>

          {/* Footer Buttons */}
          <Box display="flex" justifyContent="space-between" alignItems="center">
            <Button
              type="submit"
              variant="contained"
              color="secondary"
              sx={{ bgcolor: "#1F73F2", color: "white" }}
            >
              Create Component
            </Button>
          </Box>
        </form>
      </Paper>
  );
}

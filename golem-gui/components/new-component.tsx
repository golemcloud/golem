"use client";

import React from "react";
import { useForm, Controller } from "react-hook-form";
import {
  Box,
  Button,
  Typography,
  RadioGroup,
  FormControlLabel,
  Radio,
  TextField,
  Paper,
  IconButton,
} from "@mui/material";
import UploadFileIcon from "@mui/icons-material/UploadFile";
import FolderIcon from "@mui/icons-material/Folder";
import DeleteOutlineIcon from "@mui/icons-material/DeleteOutline";
import CloudUploadIcon from "@mui/icons-material/CloudUpload";
import { fetcher, getErrorMessage } from "@/lib/utils";
import { toast } from 'react-toastify';
import { useComponents } from "@/lib/hooks/useComponents";

type FormData = {
  name: string;
  component_type?: "0" | "1";
  component?: File | null;
  files: File[];
};

type Props = {
  mode: "create" | "update";
  onSubmitSuccess?: () => void;
  ComponentId?:string,
  initialValues?: Partial<FormData>;
};

// export default function CreateComponentForm({onCreation}:{onCreation?:()=>void}) {


export default function ComponentForm({ mode, onSubmitSuccess, initialValues,ComponentId }: Props) {
  const isCreateMode = mode === "create";
  


  const {
    handleSubmit,
    control,
    watch,
    setValue,
    getValues,
  } = useForm<FormData>({
    defaultValues: {
      name: "",
      component_type: "0",
      component: null,
      files: [],
      ...initialValues,
    },
  });

  const wasmFile = watch("component");
  const files = watch("files");
  const [error, setError] = React.useState("");

  const handleWasmUpload = (e: React.ChangeEvent<HTMLInputElement>) => {
    if (e.target.files) {
      setValue("component", e.target.files[0]);
    }
  };

  const handleFilesUpload = (e: React.ChangeEvent<HTMLInputElement>) => {
    const uploadedFiles = Array.from(e.target.files || []);
    setValue("files", [...getValues("files"), ...uploadedFiles]);
  };

  const handleFileDelete = (index: number) => {
    const updatedFiles = files.filter((_, i) => i !== index);
    setValue("files", updatedFiles);
  };

  const onSubmit = async (data: FormData) => {
    console.log("Form submitted:", data);
    try {
      const formData = new FormData();
      if(isCreateMode){
           formData.append("name", data.name);
      }
      if (isCreateMode) {
        formData.append("component_type", data.component_type || "0");
      }
      if (data.component) {
        formData.append("component", data.component);
      }

      if (data.files && data.files.length > 0) {
        data.files.forEach((file, index) => {
          formData.append(`file_${index}`, file);
        });
      }

      const endpoint = isCreateMode
      ? "?path=components"
      : `?path=components/${ComponentId}/updates`;     
      
      const response = await fetcher(endpoint, {
        method: "POST",
        body: formData,
      });

      if (response.status !== 200) {
        return setError(getErrorMessage(response.data));
      }

      setError(""); // Clear previous error
      onSubmitSuccess?.();
      isCreateMode?toast.success("Component created successfully"):toast.success("Component updated successfully");
    } catch (err) {
      console.error("Error during submission:", err);
      setError("Something went wrong! Please try again.");
    }
  };
  
  return (
    <Paper
      elevation={4}
      sx={{
        width: "100%",
        p: 2,
      }}
    >
      <form onSubmit={handleSubmit(onSubmit)}>
       { isCreateMode && <Box display="flex" gap={2} mb={3}>
          <Controller
            name="name"
            control={control}
            render={({ field }) => (
              <TextField label="Component Name" variant="outlined" fullWidth {...field} />
            )}
          />
        </Box>
        }

        {/* Type Selection */}
        {isCreateMode && (
          <Box mb={3}>
            <Typography variant="body1" mb={1}>
              Type
            </Typography>
            <Controller
              name="component_type"
              control={control}
              render={({ field }) => (
                <RadioGroup row {...field}>
                  <FormControlLabel
                    value="0"
                    control={<Radio />}
                    label={
                      <Box>
                        <Typography>
                          <b>Durable</b>
                        </Typography>
                        <Typography variant="caption" color="gray">
                          Workers are persistent and executed with transactional guarantees
                        </Typography>
                      </Box>
                    }
                  />
                  <FormControlLabel
                    value="1"
                    control={<Radio />}
                    label={
                      <Box>
                        <Typography>
                          <b>Ephemeral</b>
                        </Typography>
                        <Typography variant="caption">
                          Workers are transient and executed normally
                        </Typography>
                      </Box>
                    }
                  />
                </RadioGroup>
              )}
            />
          </Box>
        )}

        {/* WASM Binary Upload */}
        {(
          <Box
            mb={3}
            textAlign="center"
            p={2}
            border="2px dashed #444"
            borderRadius="8px"
            sx={{ cursor: "pointer" }}
          >
            <input
              type="file"
              hidden
              id="wasm-upload"
              onChange={handleWasmUpload}
              accept=".wasm"
              name="component"
            />
            <label htmlFor="wasm-upload">
              <UploadFileIcon sx={{ fontSize: 50 }} />
              <Typography variant="body2" sx={{ mt: 1 }}>
                {wasmFile ? wasmFile.name : "Upload Component WASM"}
              </Typography>
              <Typography variant="caption">File up to 50MB</Typography>
            </label>
          </Box>
        )}

        {/* Initial Files Upload */}
        <Box mb={3} p={2} border="2px dashed #444" borderRadius="8px" textAlign="center">
          <input
            type="file"
            multiple
            hidden
            id="file-upload"
            onChange={handleFilesUpload}
          />
          <label htmlFor="file-upload">
            <Typography variant="body2">Select or Drop files</Typography>
          </label>
        </Box>

        {/* File List */}
        <Box
          display="flex"
          justifyContent="space-between"
          alignItems="center"
          border="1px solid #444"
          borderRadius="8px"
          p={2}
          mb={3}
        >
          <Typography variant="caption" color="gray">
            Total Files: {files.length}
          </Typography>
          <Button variant="contained" startIcon={<FolderIcon />}>
            New Folder
          </Button>
        </Box>

        {/* Files */}
        <Box>
          {files.map((file, index) => (
            <Box
              key={index}
              display="flex"
              justifyContent="space-between"
              alignItems="center"
              p={1}
              borderBottom="1px solid #444"
            >
              <Typography variant="body2">{file.name}</Typography>
              <IconButton onClick={() => handleFileDelete(index)} color="error">
                <DeleteOutlineIcon />
              </IconButton>
            </Box>
          ))}
        </Box>

        {error && <Typography className="text-red-500">{error}</Typography>}

        {/* Submit Button */}
        <Box display="flex" justifyContent="flex-end" mt={3}>
          <Button type="submit" variant="contained" startIcon={<CloudUploadIcon />}>
            {isCreateMode ? "Create" : "Update"}
          </Button>
        </Box>
      </form>
    </Paper>
  );
}

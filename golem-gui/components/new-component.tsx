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
import { toast } from "react-toastify";
import { addNewcomponent } from "@/lib/hooks/use-component";
import { getFormErrorMessage } from "../lib/utils";

type FormData = {
  name: string;
  component_type?: "0" | "1";
  component?: File | null;
  files: File[];
};

type Props = {
  mode: "create" | "update";
  onSubmitSuccess?: () => void;
  componentId?: string;
  initialValues?: Partial<FormData>;
  getVersions?: () => any;
};

export default function ComponentForm({
  mode,
  onSubmitSuccess,
  initialValues,
  componentId,
  getVersions,
}: Props) {
  const isCreateMode = mode === "create";

  const {
    handleSubmit,
    control,
    watch,
    setValue,
    getValues,
    formState: { errors },
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
  const [error, setError] = React.useState<string | null>(null);

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
      if (isCreateMode) {
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

      const { error } = await addNewcomponent(formData, componentId, mode);
      setError(error || null); // Clear previous error
      onSubmitSuccess?.();
      if (isCreateMode) toast.success("Component created successfully");
      else toast.success("Component updated successfully");
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
        {isCreateMode && (
          <>
          <Box display="flex" gap={2}>
            <Controller
              name="name"
              rules={{ required: "Name is mandatory!" }}
              control={control}
              render={({ field }) => (
                <TextField
                  label="Component Name"
                  variant="outlined"
                  fullWidth
                  {...field}
                />
              )}
            />
          </Box>
            <Typography variant="caption" color="error">
            {getFormErrorMessage("component", errors)}
          </Typography>
          </>
        )}

        {/* Type Selection */}
        {isCreateMode && (
          <Box my={3}>
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
                          Workers are persistent and executed with transactional
                          guarantees
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
        {/* WASM File Upload */}
        <Box mb={3}>
          <Typography variant="body1">
            Upload WASM File
          </Typography>
          <Typography variant="caption" mb={1} color="gray">
          The compiled WASM binary of your component.
          </Typography>
          <Controller
            name="component"
            rules={{
              required: "WASM file is mandatory!",
              validate: (value) =>
                value?.type === "application/wasm" || "Invalid file type!",
            }}
            control={control}
            render={({ field }) => (
              <>
                <input
                  type="file"
                  accept=".wasm"
                  hidden
                  id="wasm-upload"
                  onChange={(e) =>
                    field.onChange(e.target.files ? e.target.files[0] : null)
                  }
                />
                <label htmlFor="wasm-upload">
                  <Box
                    textAlign="center"
                    p={2}
                    border="2px dashed #444"
                    borderRadius="8px"
                    sx={{ cursor: "pointer" }}
                  >
                    <UploadFileIcon sx={{ fontSize: 50 }} />
                    <Typography variant="body2" sx={{ mt: 1 }}>
                      {field.value ? field.value.name : "Upload Component WASM"}
                    </Typography>
                    <Typography variant="caption">File up to 50MB</Typography>
                  </Box>
                </label>
                <Typography variant="caption" color="error">
                  {getFormErrorMessage("component", errors)}
                </Typography>
              </>
            )}
          />
        </Box>

        {/* Initial Files Upload */}
      
        <Typography variant="body1">
            Initial Files
          </Typography>
          <Typography variant="caption" mb={1} color="gray">
          Files available to your workers at runtime.
          </Typography>
        <Box
          mb={3}
          p={2}
          border="2px dashed #444"
          borderRadius="8px"
          textAlign="center"
        >
        
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
          <Button
            type="submit"
            variant="contained"
            startIcon={<CloudUploadIcon />}
          >
            {isCreateMode ? "Create" : "Update"}
          </Button>
        </Box>
      </form>
    </Paper>
  );
}

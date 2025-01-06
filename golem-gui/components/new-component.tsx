"use client";

import React, { useCallback } from "react";
import { useForm, Controller } from "react-hook-form";
import {
  Box,
  Typography,
  RadioGroup,
  FormControlLabel,
  Radio,
  TextField,
  IconButton,
} from "@mui/material";
import UploadFileIcon from "@mui/icons-material/UploadFile";
import FolderIcon from "@mui/icons-material/Folder";
import DeleteOutlineIcon from "@mui/icons-material/DeleteOutline";
import CloudUploadIcon from "@mui/icons-material/CloudUpload";
import { addNewcomponent } from "@/lib/hooks/use-component";
import { getFormErrorMessage } from "../lib/utils";
import { Button2 } from "@/components/ui/button";
import { useDropzone } from "react-dropzone";

type FormData = {
  name: string;
  componentType?: "Durable" | "Ephemeral";
  component?: File | null;
  files: File[];
};

type Props = {
  mode: "create" | "update";
  onSubmitSuccess?: () => void;
  componentId?: string;
  initialValues?: Partial<FormData>;
};

export default function ComponentForm({
  mode,
  onSubmitSuccess,
  initialValues,
  componentId,
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
      componentType: "Durable",
      component: null,
      files: [],
      ...initialValues,
    },
  });

  const files = watch("files");
  const [error, setError] = React.useState<string | null>(null);


  const handleFileDelete = (index: number) => {
    const updatedFiles = files?.filter((_, i) => i !== index);
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
        formData.append("componentType", data.componentType || "Durable");
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
      setError(error || null);
      onSubmitSuccess?.();
    } catch (err) {
      console.error("Error during submission:", err);
      setError("Something went wrong! Please try again.");
    }
  };
  const onDrop = useCallback(
    (acceptedFiles: File[]) => {
      setValue("files", [...getValues().files, ...acceptedFiles]);
    },
    [getValues, setValue]
  );
  
  const { getRootProps, getInputProps } = useDropzone({
    onDrop,
    multiple: true,
    maxSize: 50 * 1024 * 1024, // 50MB size limit
  });

  return (
    <form onSubmit={handleSubmit(onSubmit)} className="p-5">
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
                  size="small"
                  variant="outlined"
                  fullWidth
                  {...field}
                />
              )}
            />
          </Box>
          <Typography variant="caption" color="error">
            {getFormErrorMessage("name", errors)}
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
            name="componentType"
            control={control}
            render={({ field }) => (
              <RadioGroup row {...field}>
                <FormControlLabel
                  value="Durable"
                  control={<Radio className="text-foreground" />}
                  label={
                    <Box>
                      <Typography>
                        <b>Durable</b>
                      </Typography>
                      <Typography
                        variant="caption"
                        className="text-muted-foreground"
                      >
                        Workers are persistent and executed with transactional
                        guarantees
                      </Typography>
                    </Box>
                  }
                />
                <FormControlLabel
                  value="Ephemeral"
                  control={<Radio className="text-foreground" />}
                  label={
                    <Box>
                      <Typography>
                        <b>Ephemeral</b>
                      </Typography>
                      <Typography
                        variant="caption"
                        className="text-muted-foreground"
                      >
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
        <Typography variant="body1">Upload WASM File</Typography>
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
                  borderRadius="8px"
                  className="cursor-pointer hover:border-[#888] border-dashed border-2"
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

      <Typography variant="body1">Initial Files</Typography>
      <Typography variant="caption" mb={1} color="gray">
        Files available to your workers at runtime.
      </Typography>
      <Box
        {...getRootProps()}
        textAlign="center"
        className="cursor-pointer hover:border-[#888] border-dashed border-2 p-3 mb-3 rounded-md"
      >
        <input {...getInputProps()} />
        <Typography variant="body2">Drag & Drop or Select files</Typography>
      </Box>

      {/* File List */}
      <Box
        display="flex"
        justifyContent="space-between"
        alignItems="center"
        className="cursor-pointer hover:border-[#888] border-dashed border-2 p-2 mb-3 rounded-md"
      >
        <Typography variant="caption" color="gray">
          Total Files: {files.length}
        </Typography>
        <Button2 variant="primary" size="md" startIcon={<FolderIcon />}>
          New Folder
        </Button2>
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
        <Button2
          type="submit"
          variant="primary"
          size="lg"
          startIcon={<CloudUploadIcon />}
        >
          {isCreateMode ? "Create" : "Update"}
        </Button2>
      </Box>
    </form>
  );
}

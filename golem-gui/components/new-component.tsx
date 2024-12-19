"use client";

import React, { useState } from "react";
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

export default function CreateComponentForm() {
  const [wasmFile, setWasmFile] = useState<File | null>(null);
  const [files, setFiles] = useState<File[]>([]);

  const handleWasmUpload = (e: React.ChangeEvent<HTMLInputElement>) => {
    if (e.target.files) {
      setWasmFile(e.target.files[0]);
    }
  };

  const handleFilesUpload = (e: React.ChangeEvent<HTMLInputElement>) => {
    const uploadedFiles = Array.from(e.target.files || []);
    setFiles((prev) => [...prev, ...uploadedFiles]);
  };

  const handleFileDelete = (index: number) => {
    setFiles((prev) => prev.filter((_, i) => i !== index));
  };

  return (
    <Box>
      <Paper
        elevation={3}
        sx={{
          maxWidth: "800px",
          width: "100%",
          mx: "auto",
          mt: 5,
          p: 3,
          borderRadius: "8px",
        }}
      >
        {/* Header */}
        <Typography variant="h5" sx={{ mb: 2, borderBottom: "1px solid #444", pb: 2 }}>
          Create a new Component
        </Typography>
        <Typography variant="body2" sx={{ mb: 3 }}>
          Components are the building blocks for your project
        </Typography>

        {/* Form */}
        <form>
          {/* Project and Component Name */}
          <Box display="flex" gap={2} mb={3}>
            <TextField
              label="Project"
              variant="outlined"
              fullWidth
              defaultValue="my proj"
              InputLabelProps={{
               
              }}
              InputProps={{
                sx: {borderColor: "#444" },
              }}
            />
            <TextField
              label="Component Name"
              variant="outlined"
              fullWidth
              
              InputProps={{
                sx: { borderColor: "#444" },
              }}
            />
          </Box>

          {/* Type Selection */}
          <Box mb={3}>
            <Typography variant="body1" mb={1}>
              Type
            </Typography>
            <RadioGroup row defaultValue="durable">
              <FormControlLabel
                value="durable"
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
                value="ephemeral"
                control={<Radio  />}
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
          </Box>

          {/* WASM Binary Upload */}
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
            />
            <label htmlFor="wasm-upload">
              <UploadFileIcon sx={{ fontSize: 50 }} />
              <Typography variant="body2" sx={{ mt: 1 }}>
                {wasmFile ? wasmFile.name : "Upload Component WASM"}
              </Typography>
              <Typography variant="caption" >
                File up to 50MB
              </Typography>
            </label>
          </Box>

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
            <Button
              variant="contained"
              startIcon={<FolderIcon />}
            >
              New Folder
            </Button>
          </Box>

          {/* Footer */}
          <Box display="flex" justifyContent="space-between" alignItems="center">
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
            <Button
              variant="contained"
              startIcon={<CloudUploadIcon />}
           
            >
              Create
            </Button>
          </Box>
        </form>
      </Paper>
    </Box>
  );
}

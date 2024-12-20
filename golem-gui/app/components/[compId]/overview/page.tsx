"use client";

import React, { useState } from "react";
import {
  Grid,
  Paper,
  Typography,
  Box,
  Divider,
  List,
  ListItem,
  ListItemText,
  Button,
  Dialog,
  DialogContent,
  IconButton,
} from "@mui/material";
import { InsertChart, CheckCircleOutline, ErrorOutline, RocketLaunch } from "@mui/icons-material";
import AddIcon from "@mui/icons-material/Add";
import CloseIcon from "@mui/icons-material/Close";
import CreateWorker from "@/components/create-worker"; 
import CustomModal from "@/components/CustomModal"; 


const Overview = () => {
  const stats = [
    { label: "Latest Component Version", value: "v1", icon: <InsertChart fontSize="large" /> },
    { label: "Active Workers", value: 0, icon: <CheckCircleOutline fontSize="large" /> },
    { label: "Running Workers", value: 0, icon: <RocketLaunch fontSize="large" /> },
    { label: "Failed Workers", value: 0, icon: <ErrorOutline fontSize="large" /> },
  ];

  const exports = [
    "golem:it/api.{initialize-cart}",
    "golem:it/api.{add-item}",
    "golem:it/api.{remove-item}",
    "golem:it/api.{update-item-quantity}",
    "golem:it/api.{checkout}",
    "golem:it/api.{get-cart-contents}",
  ];

  const [isOpen, setIsOpen] = useState(false);

  const handleOpen = () => setIsOpen(true);
  const handleClose = () => setIsOpen(false);

  return (
    <Box sx={{ padding: 4, minHeight: "100vh" }}>
      <Box sx={{ display: "flex", justifyContent: "flex-end" }}>
        <Button
          variant="contained"
          startIcon={<AddIcon />}
          sx={{
            textTransform: "none",
            marginLeft: "2px",
            marginBottom: "8px",
          }}
          onClick={() => {
            setIsOpen(true);
          }}
        >
          New
        </Button>
        
      </Box>

      <Grid container spacing={4}>
        {/* Stats Section */}
        {stats.map((stat, index) => (
          <Grid item xs={12} sm={6} md={3} key={index}>
            <Paper sx={{ padding: 3, textAlign: "center", bgcolor: "#1E1E1E" }}>
              {stat.icon}
              <Typography variant="h5" sx={{ marginTop: 1 }}>
                {stat.value}
              </Typography>
              <Typography variant="body1">{stat.label}</Typography>
            </Paper>
          </Grid>
        ))}

        {/* Exports Section */}
        <Grid item xs={12} md={6}>
          <Paper sx={{ padding: 3, bgcolor: "#1E1E1E" }}>
            <Typography variant="h6">Exports</Typography>
            <Divider sx={{ bgcolor: "#424242", marginY: 1 }} />
            <List>
              {exports.map((item, index) => (
                <ListItem key={index} disableGutters>
                  <ListItemText primary={item} />
                </ListItem>
              ))}
            </List>
          </Paper>
        </Grid>

        {/* Worker Status */}
        <Grid item xs={12} md={6}>
          <Paper sx={{ padding: 3, bgcolor: "#1E1E1E" }}>
            <Typography variant="h6">Worker Status</Typography>
            <Divider sx={{ bgcolor: "#424242", marginY: 1 }} />
            <Typography>No workers found</Typography>
          </Paper>
        </Grid>
      </Grid>

      <CustomModal open={isOpen} onClose={handleClose} heading="Create Worker">
        <CreateWorker/>
      </CustomModal>
    </Box>
  );
};

export default Overview;

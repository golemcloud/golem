"use client";

import React from "react";
import {
  Box,
  Typography,
  Card,
  IconButton,
  Grid as MuiGrid,
  Tooltip,
  useTheme,
  Paper,
} from "@mui/material";
import DescriptionIcon from "@mui/icons-material/Description";
import WorkIcon from "@mui/icons-material/Work";
import SettingsIcon from "@mui/icons-material/Settings";
import CodeIcon from "@mui/icons-material/Code";
import BuildIcon from "@mui/icons-material/Build";
import CloudIcon from "@mui/icons-material/Cloud";
import PeopleAltIcon from "@mui/icons-material/PeopleAlt";
import { useRouter } from "next/navigation";

const Dashboard = () => {
  const router = useRouter();

  const apis = [
    { id: 1, name: "My Project A", components: 2, apis: 3 },
    { id: 2, name: "My Project B", components: 4, apis: 0 },
    { id: 3, name: "Project C", components: 1, apis: 2 },
  ];

  const buttonData = [
    {
      label: "Docs",
      icon: <DescriptionIcon />,
      onClick: () => router.push("/docs"),
    },
    {
      label: "Overview",
      icon: <WorkIcon />,
      onClick: () => router.push("/overview"),
    },
    {
      label: "Settings",
      icon: <SettingsIcon />,
      onClick: () => router.push("/settings"),
    },
  ];

  const resources = [
    {
      label: "Language Guides",
      icon: <CodeIcon />,
      description: "Choose your language and start building",
    },
    {
      label: "Components",
      icon: <BuildIcon />,
      description: "Create WASM components that run on Golem",
    },
    {
      label: "APIs",
      icon: <CloudIcon />,
      description: "Craft custom APIs to expose your components to the world",
    },
    {
      label: "Workers",
      icon: <PeopleAltIcon />,
      description: "Launch and manage efficient workers from your components",
    },
  ];

  return (
    <Box className="container mx-auto flex flex-col gap-8 px-4 py-8 md:px-6 lg:px-8">
      {/* Hero Section */}
      <Paper
        elevation={2}
        sx={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          p: 3,
          borderRadius: "5px",
          border: "0.1px solid #555",
        }}
      >
        <Box>
          <Typography variant="h4" sx={{ fontWeight: 700 }}>
            Welcome, Mubashir Shariq
          </Typography>
          <Typography variant="h6" sx={{ mt: 1 }}>
            Here is a quick overview of your account
          </Typography>
        </Box>
        <Box textAlign="center">
          <Typography
            variant="h2"
            sx={{
              fontWeight: 900,
            }}
          >
            {apis.length}
          </Typography>
          <Typography variant="subtitle2" sx={{ fontWeight: 500 }}>
            APIs Created
          </Typography>
        </Box>
      </Paper>
      {/* Quick Access Section */}
      <Paper
        sx={{
          display: "flex",
          flexDirection: "column",
          mb: 2,
          py: 4,
          px: 3,
          borderRadius: "5px",
          border: "0.1px solid #555",
        }}
      >
        <Typography variant="h5" sx={{ mb: 2, fontWeight: 600 }}>
          Quick Access
        </Typography>
        <Box>
          <MuiGrid container spacing={4}>
            {buttonData.map((item) => (
              <MuiGrid item key={item.label}>
                <IconButton
                  onClick={item.onClick}
                  className="dark:text-white hover:bg-accent border-[var(--border)]"
                  sx={{
                    display: "flex",
                    flexDirection: "column",
                    alignItems: "center",
                    justifyContent: "center",
                    height: "100px",
                    width: "100px",
                    padding: "1.5rem",
                    borderRadius: "5px",
                    transition: "transform 0.3s ease, 0.3s ease",
                    "&:hover": {
                      // transform: "translateY(-5px)",
                      // border:'0px solid #555',
                      backgroundColor:'#555'
                    },
                  }}
                >
                  {item.icon}
                  <Typography variant="caption" sx={{ mt: 1, fontWeight: 500 }}>
                    {item.label}
                  </Typography>
                </IconButton>
              </MuiGrid>
            ))}
          </MuiGrid>
        </Box>
      </Paper>
      {/* Resources Section */}
      <Box>
        <Typography variant="h5" sx={{ mb: 3, fontWeight: 600 }}>
          Resources
        </Typography>
        <MuiGrid container spacing={4}>
          {resources.map((resource) => (
            <MuiGrid item xs={12} sm={6} md={3} key={resource.label}>
              <Card
                sx={{
                  display: "flex",
                  flexDirection: "column",
                  justifyContent: "space-between",
                  padding: "1.5rem",
                  width: "100%",
                  height: "200px",
                  background: "rgba(0, 0, 0, 0.05)",
                  borderRadius: "20px",
                  textAlign: "center",
                  boxShadow: "0 6px 20px rgba(0, 0, 0, 0.1)",
                  backdropFilter: "blur(15px)",
                  transition: "transform 0.3s ease, box-shadow 0.3s ease",
                  border:'0.1px solid #555',
                  "&:hover": {
                    transform: "translateY(-5px)",
                    boxShadow: "0 12px 35px rgba(0, 0, 0, 0.2)",
                    border:'0px solid #555',
                  },
                }}
              >
                <Box
                  sx={{
                    display: "flex",
                    alignItems: "center",
                    justifyContent: "space-between",
                    gap: "5rem",
                  }}
                >
                  <Typography variant="h6" sx={{ fontWeight: 600 }}>
                    {resource.label}
                  </Typography>
                  <Box sx={{ fontSize: "2.5rem", color: "#ff9800" }}>
                    {resource.icon}
                  </Box>
                </Box>
                <Typography
                  variant="body2"
                  sx={{
                    color: "rgba(0,0,0,0.6)",
                  }}
                >
                  {resource.description}
                </Typography>
              </Card>
            </MuiGrid>
          ))}
        </MuiGrid>
      </Box>
    </Box>
  );
};

export default Dashboard;

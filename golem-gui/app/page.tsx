"use client";

import React from "react";
import {
  Box,
  Typography,
  Card,
  IconButton,
  Grid2 as MuiGrid,
  Tooltip,
  useTheme,
  Paper,
} from "@mui/material";
import DescriptionIcon from "@mui/icons-material/Description";
import WorkIcon from "@mui/icons-material/Work";
import SettingsIcon from "@mui/icons-material/Settings";
import { NotepadText, Code, BookOpenText, Github } from "lucide-react";

import { useRouter } from "next/navigation";
import useComponents from "@/lib/hooks/use-component";
import useApiDefinitions from "@/lib/hooks/use-api-definitons";
import FooterLinks from "@/components/ui/footer-links";

const Dashboard = () => {
  const router = useRouter();
  const { components } = useComponents();
  const { apiDefinitions } = useApiDefinitions();
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
      label: "Getting Started",
      icon: <BookOpenText />,
      description:
        "Learn how to setup your development environment and build your first component",
    },
    {
      label: "API Docs",
      icon: <Code />,
      description:
        "Explore the API Documentation and learn how to integrate with our platform",
    },
    {
      label: "Language Guides",
      icon: <NotepadText />,
      description:
        "Check out our language specific tutorials and examples to get started",
    },
    {
      label: "Github",
      icon: <Github />,
      description:
        "Check out our Github repository to contribute and report issues",
    },
  ];

  return (
    <main className="container mx-auto flex flex-col gap-8 px-4 py-8 md:px-6 lg:px-8">
      <Paper
        elevation={2}
        sx={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          p: 3,
          borderRadius: "5px",
        }}
        className="border"
      >
        <Box>
          <Typography variant="h4" sx={{ fontWeight: 700 }}>
            Welcome, Mubashir Shariq
          </Typography>
          <Typography variant="h6" className="text-muted-foreground mt-1">
            Here is a quick overview of your account
          </Typography>
        </Box>
        <Box className="flex gap-3 pr-3">
          <Box textAlign="center">
            <Typography
              variant="h2"
              sx={{
                fontWeight: 900,
              }}
            >
              {components.length}
            </Typography>
            <Typography variant="subtitle2" sx={{ fontWeight: 500 }}>
              Components
            </Typography>
          </Box>
          <Box textAlign="center">
            <Typography
              variant="h2"
              sx={{
                fontWeight: 900,
              }}
            >
              {apiDefinitions.length}
            </Typography>
            <Typography variant="subtitle2" sx={{ fontWeight: 500 }}>
              Api's
            </Typography>
          </Box>
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
        }}
        className="border"
      >
        <Typography variant="h5" sx={{ mb: 2, fontWeight: 600 }}>
          Quick Access
        </Typography>
        <Box>
          <MuiGrid container spacing={4}>
            {buttonData.map((item) => (
              <MuiGrid
                className=" border dark:hover:bg-[#555] rounded-md"
                key={item.label}
              >
                <IconButton
                  onClick={item.onClick}
                  color="inherit"
                  className="dark:text-white"
                  sx={{
                    display: "flex",
                    flexDirection: "column",
                    alignItems: "center",
                    justifyContent: "center",
                    height: "100px",
                    width: "100px",
                    padding: "1.5rem",
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
      <FooterLinks variant="main" resources={resources} />
    </main>
  );
};

export default Dashboard;

'use client';

import React from "react";
import {
  Box,
  Typography,
  Card,
  CardContent,
  Button,
  Grid as MuiGrid,
  Tooltip,
} from "@mui/material";
import DescriptionIcon from "@mui/icons-material/Description";
import WorkIcon from "@mui/icons-material/Work";
import SettingsIcon from "@mui/icons-material/Settings";
import OverviewFooter from "@/components/ui/overview-footer";
import { useRouter } from "next/navigation";

const Dashboard = () => {
  const router = useRouter();

  // Mock data for projects
  const apis = [
    { id: 1, name: "My Project A", components: 2, apis: 3 },
    { id: 2, name: "My Project B", components: 4, apis: 0 },
    { id: 3, name: "Project C", components: 1, apis: 2 },
  ];

  const buttonData = [
    { label: "Docs", icon: <DescriptionIcon />, onClick: () => router.push("/docs") },
    { label: "Overview", icon: <WorkIcon />, onClick: () => router.push("/overview") },
    { label: "Settings", icon: <SettingsIcon />, onClick: () => router.push("/settings") },
  ];


  return (
    <Box sx={{ flexGrow: 1, padding: "2rem" }}>
      <Box sx={{ display: "flex", justifyContent: "space-between", mb: 4 }}>
        <Card sx={{ flex: 1, mr: 2 }}>
          <CardContent>
            <Typography variant="h6">Welcome, Mubashir Shariq</Typography>
            <Typography variant="body2" sx={{ mt: 1 }}>
              Here's a quick overview of your account.
            </Typography>
            <Typography variant="h4" sx={{ mt: 2 }}>
              {apis.length}
            </Typography>
            <Typography variant="caption">Apis</Typography>
          </CardContent>
        </Card>
        <Card sx={{ flex: 1 }}>
          <CardContent>
            <Typography variant="h6">Quick Access</Typography>
            <MuiGrid container spacing={2} sx={{ mt: 1 }}>
              {buttonData.map((item) => (
                <MuiGrid item xs={4} key={item.label}>
                  <Tooltip title={item.label} arrow>
                    <Button variant="outlined" fullWidth startIcon={item.icon} onClick={item.onClick}>
                      {item.label}
                    </Button>
                  </Tooltip>
                </MuiGrid>
              ))}
            </MuiGrid>
          </CardContent>
        </Card>
      </Box>
      
      <Box>
        <Typography variant="h6" sx={{ mb: 2 }}>
          Resources
        </Typography>
        <OverviewFooter />
      </Box>
    </Box>
  );
};

export default Dashboard;

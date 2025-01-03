"use client";

import React from "react";
import { Typography, Box, Divider } from "@mui/material";
import {Button2 as Button} from "@/components/ui/button";

type SettingProps = {
    title: string;
    description: string;
    actions: {
        title: string;
        description: string;
        buttonText: string;
        disabled?: boolean
        onClick: (e:React.MouseEvent<HTMLButtonElement>) => void;
    }[];
    };
const DangerZone = ({ title, description, actions }: SettingProps) => {
  return (
    <div className=" p-4">
      <Box className="flex flex-col">
        <Typography variant="h6">{title}</Typography>
        <Typography variant="subtitle1" gutterBottom className="text-muted-foreground">
            {description}
        </Typography>
      </Box>

      <Divider className="my-2 bg-border" />

      {/* Danger Zone Section */}
      <div className="bg-white dark:bg-[#281619] p-6 rounded-lg shadow-lg border dark:border-[#6a1d25] border-[#f04444]">
        <h2 className="text-lg font-bold text-foreground mb-2">Danger Zone</h2>
        <Divider className="my-2 bg-border" />
        <p className="text-sm text-muted-foreground mb-4">Proceed with caution</p>

        {actions.map((action, index) => (
          <div key={index} className="flex justify-between items-center mb-4">
            <div>
              <h3 className="text-md  text-foreground">
                {action.title}
              </h3>
              <p className="text-muted-foreground text-sm">{action.description}</p>
            </div>
            <Button
              variant="error"
              className="mt-2"
              size="md"
              onClick={action.onClick}
            >
              {action.buttonText}
            </Button>

          </div>
        ))}
      </div>
    </div>
  );
};

export default DangerZone;

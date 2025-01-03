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
      <div className="bg-white dark:bg-[#3B191D80] p-6 rounded-lg shadow-lg border border-red-500">
        <h2 className="text-xl font-bold text-foreground mb-4">{title}</h2>
        <Divider className="my-2 bg-border" />
        <p className="text-muted-foreground mb-4">{description}</p>

        {actions.map((action, index) => (
          <div key={index} className="flex justify-between items-center mb-4">
            <div>
              <h3 className="text-lg font-semibold text-foreground">
                {action.title}
              </h3>
              <p className="text-muted-foreground">{action.description}</p>
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

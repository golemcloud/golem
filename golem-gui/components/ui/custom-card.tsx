import { Box, Card, CardContent, Typography, Chip } from "@mui/material";

interface CustomCardProps {
  title: string;
  time: number;
  version: number;
  exports: number;
  size: string;
  componentType: string;
}
const CustomCard = ({
  title,
  time,
  exports,
  size,
  componentType,
  version,
}: CustomCardProps) => {
  return (
    <Card
      sx={{
        width:350,
        padding: 1,
      }}
    >
      <CardContent>
        <Box sx={{ display: "flex", justifyContent: "space-between" }}>
          <Typography variant="inherit" component="div" gutterBottom>
            {title}
          </Typography>
          <Typography
          className=" bg-[#787676] text-white px-2 py-1 rounded-md text-sm"
         >
            v{version}
          </Typography>
        </Box>
        <Typography variant="body2"
            className="text-[#555] dark:text-gray-300 mb-3"
        >{time} hours ago</Typography>
        <Box sx={{ display: "flex", gap: 1, alignItems: "center"}}
        >
           <Typography variant="body1" className="border border-[#555] px-2 rounded-md">
           {`${exports} Exports`} 
            </Typography> 
            <Typography variant="body1" className="border border-[#555] px-2 rounded-md">
            {`${size} MB`}
            </Typography> 
            <Typography variant="body1" className="border border-[#555] px-2 rounded-md">
            {componentType} 
            </Typography> 
        </Box>
      </CardContent>
    </Card>
  );
};

export default CustomCard;

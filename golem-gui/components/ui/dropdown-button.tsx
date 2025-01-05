import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import ArrowDropDownIcon from "@mui/icons-material/ArrowDropDown";
import { Typography } from "@mui/material";
import { useRouter } from "next/navigation";
import { Button2 } from "./button";

export function Dropdown(list: { route: string; value: string }[]) {
  const router = useRouter();

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <ArrowDropDownIcon />
      </DropdownMenuTrigger>
      <DropdownMenuContent className="w-16">
        <DropdownMenuGroup>
          {list.map((item, ind) => (
            <DropdownMenuItem
              key={ind}
              onClick={(e) => {
                e.stopPropagation();
                router.push(item.route);
              }}
              className="cursor-pointer"
            >
              {item.value}
            </DropdownMenuItem>
          ))}
        </DropdownMenuGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

export function DropdownV2({
  list,
  prefix,
  icon,
}: {
  list: {
    label: string;
    value: string | number;
    onClick?: (value: string | number) => void;
  }[];
  prefix?: string;
  icon?: React.ReactNode;
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger>
        {prefix && (
          <Button2 variant={"outline"} size="md" endIcon={icon || <ArrowDropDownIcon />}>
            <Typography>{prefix}</Typography>
          </Button2>
        )}
      </DropdownMenuTrigger>
      <DropdownMenuContent className="w-16">
        <DropdownMenuGroup>
          {list.map((item, ind) => (
            <DropdownMenuItem
              key={ind}
              onClick={(e) => {
                e.stopPropagation();
                if (item?.onClick) {
                  item.onClick(item.value);
                }
              }}
              className="cursor-pointer"
            >
              {item.label}
            </DropdownMenuItem>
          ))}
        </DropdownMenuGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

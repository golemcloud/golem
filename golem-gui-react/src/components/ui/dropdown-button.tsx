import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@ui/dropdown-menu";
import ArrowDropDownIcon from "@mui/icons-material/ArrowDropDown";
import { useNavigate } from "react-router-dom";

export function Dropdown(list: { route: string; value: string }[]) {
  const navigate = useNavigate();

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
                navigate(item.route);
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
  icon
}: {
  list: {
    label: string;
    value: string | number;
    onClick?: (value: string | number) => void;
    disabled?:boolean
  }[];
  prefix?: string;
  icon?: React.ReactNode;
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger>
        {prefix && (
         <div
         className="inline-flex items-center justify-between px-4 py-2 border rounded-md cursor-pointer 
           bg-white text-gray-800 hover:bg-gray-100 
           dark:bg-gray-800 dark:text-white dark:hover:bg-gray-700"
       >
         <span>{prefix}</span>
         {icon || <ArrowDropDownIcon />}
       </div>
                
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
              disabled={item.disabled}
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

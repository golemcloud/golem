import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import ArrowDropDownIcon from "@mui/icons-material/ArrowDropDown";
import Link from "next/link";

export function Dropdown(list: { route: string; value: string }[]) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <ArrowDropDownIcon />
      </DropdownMenuTrigger>
      <DropdownMenuContent className="w-16">
        <DropdownMenuGroup>
          {list.map((item) => (
            <DropdownMenuItem>
              <Link href={item.route}>{item.value}</Link>
            </DropdownMenuItem>
          ))}
        </DropdownMenuGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

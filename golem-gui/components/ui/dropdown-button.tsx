import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import ArrowDropDownIcon from "@mui/icons-material/ArrowDropDown";
import { useRouter } from "next/navigation";

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
              onClick={() => router.push(item.route)}
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

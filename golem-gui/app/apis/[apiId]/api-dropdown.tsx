import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import ArrowDropDownIcon from "@mui/icons-material/ArrowDropDown";

interface list {
  onClick: () => void;
  label: string;
}
interface DropdownGroup {
  heading: string;
  list: list[];
}

interface ApiDropdownProps {
  dropdowns: DropdownGroup[];
}

export function ApiDropdown({ dropdowns }: ApiDropdownProps) {

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <ArrowDropDownIcon />
      </DropdownMenuTrigger>
      <DropdownMenuContent className="w-56">
        {dropdowns.map((dropdown, index) => (
          <div key={index}>
            <DropdownMenuLabel className="text-muted-foreground">{dropdown.heading}</DropdownMenuLabel>
            <DropdownMenuSeparator />
            <DropdownMenuGroup>
              {dropdown.list.map((item, idx) => (
                <DropdownMenuItem
                  key={idx}
                  onClick={item.onClick}
                  className="cursor-pointer"
                >
                  {item.label}
                </DropdownMenuItem>
              ))}
            </DropdownMenuGroup>
            {index < dropdowns.length - 1 && <DropdownMenuSeparator />}
          </div>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

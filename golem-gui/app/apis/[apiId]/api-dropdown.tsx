import CustomModal from "@/components/CustomModal";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuPortal,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import ArrowDropDownIcon from "@mui/icons-material/ArrowDropDown";
import { useRouter } from "next/navigation";

interface list {
  route: string;
  value: string;
}
interface DropdownGroup {
  heading: string;
  list: list[];
}

interface ApiDropdownProps {
  dropdowns: DropdownGroup[];
}

export function ApiDropdown({ dropdowns }: ApiDropdownProps) {
  const router = useRouter();

  return (
    <>
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <ArrowDropDownIcon />
      </DropdownMenuTrigger>
      <DropdownMenuContent className="w-56">
        {dropdowns.map((dropdown, index) => (
          <div key={index}>
            <DropdownMenuLabel>{dropdown.heading}</DropdownMenuLabel>
            <DropdownMenuSeparator />
            <DropdownMenuGroup>
              {dropdown.list.map((item, idx) => (
                <DropdownMenuItem
                  key={idx}
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
            {index < dropdowns.length - 1 && <DropdownMenuSeparator />}
          </div>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>

    {/* <CustomModal open={!!open} onClose={handleClose}>
    {open === "api" && <CreateAPI onCreation={handleClose} />}
    {open === "component" && (
      <CreateComponentForm mode="create" onSubmitSuccess={handleClose} />
    )}
    </CustomModal> */}
  </>
  );

}

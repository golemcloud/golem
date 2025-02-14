import { Plus, Share2, Tags, Trash2, Upload } from "lucide-react";

import toast from "react-hot-toast";
import { useDeleteApiDefinition } from "../../api/api-definitions";
import { useMutation } from "@tanstack/react-query";
import { useNavigate } from "react-router-dom";

interface ActionButtonProps {
    children: React.ReactNode;
    onClick: () => void;
    variant?: "primary" | "success" | "danger" | "default";
    className?: string;
}

const ActionButton = ({ children, onClick, variant = "default", className = "" }: ActionButtonProps) => {
    const baseStyles = "flex items-center justify-center gap-2 px-4 py-2 rounded transition-colors";
    const variantStyles = {
        primary: "bg-primary text-primary-foreground hover:bg-primary/90",
        success: "bg-success text-success-foreground hover:bg-success/90",
        danger: "bg-destructive text-destructive-foreground hover:bg-destructive/90",
        default: "bg-card hover:bg-muted"
    };

    return (
        <button
            onClick={onClick}
            className={`${baseStyles} ${variantStyles[variant]} ${className}`}
        >
            {children}
        </button>
    );
};

interface ActionButtonsProps {
    apiDefinition: {
        id: string;
        version: string;
        draft: boolean;
    };
    onPublish: () => void;
    onDeploy: () => void;
    onAddRoute: () => void;
    onNewVersion: () => void;
    showMobileMenu: boolean;
    className?: string;
}

const ActionButtons = ({
    apiDefinition,
    onPublish,
    onDeploy,
    onAddRoute,
    onNewVersion,
    showMobileMenu,
    className = ""
}: ActionButtonsProps) => {
    // const navigate = useNavigate();
    const deleteApiDefinition = useDeleteApiDefinition();

    const handleDelete = () => {
        if (window.confirm("Are you sure you want to delete this API definition? This action cannot be undone.")) {
            deleteApiDefinition.mutate();
        }
    };

    const ButtonGroup = ({ children }: { children: React.ReactNode }) => (
        <div className="flex gap-2">{children}</div>
    );

    return (
        <div className={`flex flex-col sm:flex-row gap-2 ${showMobileMenu ? "block" : "hidden md:flex"} ${className}`}>
            {/* Version Control Group */}
            <ButtonGroup>
                {apiDefinition.draft && (
                    <ActionButton onClick={onPublish} variant="primary">
                        <Share2 size={18} />
                        <span>Publish</span>
                    </ActionButton>
                )}
                <ActionButton onClick={onNewVersion} variant="primary">
                    <Tags size={18} />
                    <span>New Version</span>
                </ActionButton>

            </ButtonGroup>

            {/* Operations Group */}
            <ButtonGroup>
                <ActionButton onClick={onDeploy} variant="success">
                    <Upload size={18} />
                    <span>Deploy</span>
                </ActionButton>
                {apiDefinition.draft && (
                    <ActionButton onClick={onAddRoute} variant="primary">
                        <Plus size={18} />
                        <span>Add Route</span>
                    </ActionButton>
                )}
            </ButtonGroup>

            {/* Danger Zone */}
            <ButtonGroup>
                <ActionButton onClick={handleDelete} variant="danger">
                    <Trash2 size={18} />
                    <span>Delete</span>
                </ActionButton>
            </ButtonGroup>
        </div>
    );
};

export default ActionButtons;
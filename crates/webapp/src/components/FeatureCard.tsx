import React from 'react';
import { LucideIcon } from 'lucide-react';

interface FeatureCardProps {
  title: string;
  description: string;
  Icon: LucideIcon;
  onClick: () => void;
}

const FeatureCard = ({ title, description, Icon, onClick }: FeatureCardProps) => {
  return (
    <div className="bg-white rounded-lg border border-gray-200 p-6 cursor-pointer hover:bg-gray-50" onClick={onClick}>
      <div className="mb-4">
        <Icon className="h-8 w-8 text-gray-600" />
      </div>
      <h3 className="text-lg font-medium mb-2">{title}</h3>
      <p className="text-gray-600">{description}</p>
    </div>
  );
};

export default FeatureCard;
import React from 'react';
import { FileText, Box, Globe, Cpu } from 'lucide-react';
import APISection from './APISection';
import ComponentsSection from './ComponentsSection';
import FeatureCard from '../../components/FeatureCard';

const Overview = () => {
  return (
    <div className="container mx-auto px-4 py-8">
      <div className="grid grid-cols-1 md:grid-cols-2 gap-6 mb-8">
        <APISection />
        <ComponentsSection />
      </div>
      
      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-6">
        <FeatureCard
          onClick={() => window.open("https://learn.golem.cloud/docs/develop-overview")}
          Icon={FileText}
          title="Language Guides"
          description="Choose your language and start building"
        />
        <FeatureCard
          onClick={() => window.open("https://learn.golem.cloud/docs/concepts/components")}
          Icon={Box}
          title="Components"
          description="Create WASM components that run on Golem"
        />
        <FeatureCard
          onClick={() => window.open("https://learn.golem.cloud/docs/concepts/apis")}
          Icon={Globe}
          title="APIs"
          description="Craft custom APIs to expose your components to the world"
        />
        <FeatureCard
          onClick={() => window.open("https://learn.golem.cloud/docs/concepts/workers")}
          Icon={Cpu}
          title="Workers"
          description="Launch and manage efficient workers from your components"
        />
      </div>
    </div>
  );
};

export default Overview;
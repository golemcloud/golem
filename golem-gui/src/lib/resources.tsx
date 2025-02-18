
import { NotepadText, Component, Globe, Bot } from "lucide-react";

export const resources = [
  {
    label: "Language Guides",
    icon: <NotepadText />,
    description: "Check our language and start building",
    link: "https://learn.golem.cloud/docs/develop-overview",
  },
  {
    label: "Components",
    icon: <Component />,
    description: "Create Wasm components that run on Golem",
    link: "https://learn.golem.cloud/docs/concepts/components",
  },
  {
    label: "APIs",
    icon: <Globe />,
    description: "Craft custom APIs to expose your components to the world",
    link: "https://learn.golem.cloud/docs/rest-api/oss-rest-api",
  },
  {
    label: "Workers",
    icon: <Bot />,
    description: "Launch and manage efficient workers from your components",
    link: "https://learn.golem.cloud/docs/concepts/workers",
  },
];


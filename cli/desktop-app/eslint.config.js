import globals from "globals";
import pluginJs from "@eslint/js";
import tseslint from "typescript-eslint";
import pluginReact from "eslint-plugin-react";
import pluginReactRefresh from "eslint-plugin-react-refresh";

export default [
  // JavaScript files
  {
    files: ["**/*.{js,mjs,cjs,jsx}"],
    languageOptions: {
      parserOptions: {
        ecmaFeatures: {
          jsx: true,
        },
        ecmaVersion: "latest",
        sourceType: "module",
      },
      globals: {
        ...globals.browser,
        ...globals.node,
        // Vitest globals
        vi: "readonly",
        describe: "readonly",
        it: "readonly",
        expect: "readonly",
        beforeEach: "readonly",
        afterEach: "readonly",
        afterAll: "readonly",
        beforeAll: "readonly",
      },
    },
    plugins: {
      react: pluginReact,
      "react-refresh": pluginReactRefresh,
    },
    rules: {
      ...pluginJs.configs.recommended.rules,
      ...pluginReact.configs.recommended.rules,
      "react/react-in-jsx-scope": "off",
      "react/jsx-uses-react": "off",
      "react/jsx-no-target-blank": "off",
      "react-refresh/only-export-components": [
        "off",
        { allowConstantExport: true },
      ],
      "react/prop-types": "off",
      "react/no-unknown-property": [
        "error",
        { ignore: ["cmdk-input-wrapper"] },
      ],
      "no-useless-escape": "off",
    },
    settings: {
      react: {
        version: "detect",
      },
    },
  },
  // TypeScript files
  {
    files: ["**/*.{ts,tsx}"],
    languageOptions: {
      parser: tseslint.parser,
      parserOptions: {
        ecmaFeatures: {
          jsx: true,
        },
        ecmaVersion: "latest",
        sourceType: "module",
        project: "./tsconfig.json",
        tsconfigRootDir: import.meta.dirname,
      },
      globals: {
        ...globals.browser,
        ...globals.node,
        // Vitest globals
        vi: "readonly",
        describe: "readonly",
        it: "readonly",
        expect: "readonly",
        beforeEach: "readonly",
        afterEach: "readonly",
        afterAll: "readonly",
        beforeAll: "readonly",
      },
    },
    plugins: {
      "@typescript-eslint": tseslint.plugin,
      react: pluginReact,
      "react-refresh": pluginReactRefresh,
    },
    rules: {
      ...pluginJs.configs.recommended.rules,
      ...tseslint.configs.recommended.rules,
      ...pluginReact.configs.recommended.rules,
      "react/react-in-jsx-scope": "off", // React 17+ doesn't require React to be in scope
      "react/jsx-uses-react": "off", // React 17+ doesn't require React to be in scope
      "react/jsx-no-target-blank": "off",
      "react-refresh/only-export-components": [
        "off",
        { allowConstantExport: true },
      ],
      // Disable prop-types as it's not used in this project
      "react/prop-types": "off",
      // Allow custom attributes for libraries like cmdk
      "react/no-unknown-property": [
        "error",
        { ignore: ["cmdk-input-wrapper"] },
      ],
      // Disable base ESLint no-useless-escape
      "no-useless-escape": "off",
      // TypeScript ESLint rules
      "@typescript-eslint/no-unused-vars": [
        "warn",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
      ],
      "@typescript-eslint/ban-ts-comment": "off",
      "@typescript-eslint/no-explicit-any": "error", // Prevent usage of any type
      // Disable base ESLint no-unused-vars to avoid conflicts with TypeScript version
      "no-unused-vars": "off",
      // Disable base ESLint no-undef to avoid conflicts with TypeScript version
      "no-undef": "off",
      // Disable no-redeclare and no-constant-binary-expression
      "no-redeclare": "off",
      "no-constant-binary-expression": "off",
      "no-unreachable": "off",
      "no-import-assign": "off",
    },
    settings: {
      react: {
        version: "detect",
      },
    },
  },
  {
    files: ["vite.config.ts", "vitest.config.ts"],
    languageOptions: {
      parser: tseslint.parser,
      parserOptions: {
        ecmaVersion: "latest",
        sourceType: "module",
        project: "./tsconfig.node.json",
        tsconfigRootDir: import.meta.dirname,
      },
      globals: {
        ...globals.node,
      },
    },
    plugins: {
      "@typescript-eslint": tseslint.plugin,
    },
    rules: {
      ...pluginJs.configs.recommended.rules,
      ...tseslint.configs.recommended.rules,
      "@typescript-eslint/no-unused-vars": [
        "warn",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
      ],
      "@typescript-eslint/ban-ts-comment": "off",
      "@typescript-eslint/no-explicit-any": "error",
      "no-unused-vars": "off",
      "no-undef": "off",
      "no-redeclare": "off",
      "no-constant-binary-expression": "off",
      "no-unreachable": "off",
      "no-import-assign": "off",
    },
  },
  {
    ignores: ["dist/", "node_modules/", "src-tauri/"],
  },
];

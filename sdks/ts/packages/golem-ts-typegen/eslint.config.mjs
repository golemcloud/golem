import pluginTs from '@typescript-eslint/eslint-plugin';
import parserTs from '@typescript-eslint/parser';
import globals from 'globals';

export default [
    {
        ignores: ['.metadata/**', 'tests/**', 'dist/**', 'types/**'],
    },
    {
        files: ['src/**/*.{ts,tsx}'],
        languageOptions: {
            parser: parserTs,
            parserOptions: {
                project: './tsconfig.json',
                sourceType: 'module',
            },
            globals: {
                ...globals.node,
            },
        },
        plugins: { '@typescript-eslint': pluginTs },
        rules: {
            'no-unused-vars': 'warn',
            'no-undef': 'error',
            'no-unreachable': 'error',
            'no-dupe-keys': 'error',
            'no-duplicate-case': 'error',
            eqeqeq: ['error', 'always'],
            '@typescript-eslint/no-floating-promises': 'error',
            '@typescript-eslint/no-misused-promises': 'error',
        },
    },
];

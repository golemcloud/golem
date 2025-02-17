import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import path from 'path'

// https://vite.dev/config/
export default defineConfig({
  plugins: [react(),tailwindcss()],
  resolve: {
    alias: {
      "@components": path.resolve(__dirname, "src/components"),
      "@pages": path.resolve(__dirname, "src/pages"),
      "@utils": path.resolve(__dirname, "src/utils"),
      "@assets": path.resolve(__dirname, "src/assets"),
      "@lib": path.resolve(__dirname, "src/lib"),
      "@services": path.resolve(__dirname, "src/services"),
      "@ui": path.resolve(__dirname, "src/components/ui"),
    },
  },
  define: {
    'process.env': {},
  }
})

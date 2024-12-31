/** @type {import('next').NextConfig} */
const nextConfig = {
    async rewrites() {
      return [
        {  
          source: '/api-backend/:path*', // Match API routes
          destination: 'http://localhost:9881/:path*', // Proxy to backend. move it to env
        },
      ];
    },
  };
export default nextConfig;
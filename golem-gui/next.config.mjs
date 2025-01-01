/** @type {import('next').NextConfig} */
const nextConfig = {
  async rewrites() {
    return [
      {  
        source: '/api-backend/:path*', // Match API routes
        destination: `${process.env.BACKEND_API_URL}/:path*`, // Use environment variable
      },
    ];
  },
};

export default nextConfig;

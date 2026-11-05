import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  allowedDevOrigins: ["*.*", "**.*"],
  serverExternalPackages: ["@grpc/grpc-js"],
  async redirects() {
    return [
      { source: "/hrpd", destination: "/evdo", permanent: true },
      { source: "/hrpd/:uati", destination: "/evdo/:uati", permanent: true },
    ];
  },
};

export default nextConfig;

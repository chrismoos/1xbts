import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  allowedDevOrigins: ["*.*", "**.*"],
  serverExternalPackages: ["@grpc/grpc-js"],
};

export default nextConfig;

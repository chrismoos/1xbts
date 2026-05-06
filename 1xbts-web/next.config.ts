import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  serverExternalPackages: ["@grpc/grpc-js"],
};

export default nextConfig;

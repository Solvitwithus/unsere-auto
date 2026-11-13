import type { NextConfig } from "next";

// Static export: `next build` writes plain HTML/JS/CSS to ./out, which
// wb-server (dashboard) and wb-station (operator screen) serve directly.
// No Node.js is needed in production.
const nextConfig: NextConfig = {
  output: "export",
  trailingSlash: true, // /tickets/ -> out/tickets/index.html, served as a directory
  images: { unoptimized: true },
};

export default nextConfig;

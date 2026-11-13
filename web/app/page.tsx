"use client";
// Entry point: a station install opens the operator screen, the central server the dashboard.
import { useEffect } from "react";
import { api } from "@/lib/api";

export default function Home() {
  useEffect(() => {
    api("/api/station/status")
      .then(() => window.location.replace("/station/"))
      .catch(() => window.location.replace("/dashboard/"));
  }, []);
  return <div className="main muted">Loading...</div>;
}

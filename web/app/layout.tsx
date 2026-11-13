import "./globals.css";
import type { Metadata } from "next";

export const metadata: Metadata = { title: "Weighbridge", description: "Weighbridge tickets, sales and stations" };

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  );
}

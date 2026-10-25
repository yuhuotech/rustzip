import type { Metadata } from "next";
import { SiteFooter } from "@/components/site-footer";
import { SiteHeader } from "@/components/site-header";
import "./globals.css";

export const metadata: Metadata = {
  metadataBase: new URL(
    process.env.SITE_URL ??
      `https://${process.env.VERCEL_PROJECT_PRODUCTION_URL ?? process.env.VERCEL_URL ?? "rustzip.vercel.app"}`,
  ),
  title: {
    default: "RustZIP — 快速、安全、开源的压缩管理工具",
    template: "%s · RustZIP",
  },
  description:
    "RustZIP 是一款用 Rust 构建、面向 Windows、macOS 和 Linux 开发的压缩管理软件，支持浏览、压缩、解压、转换和修复。",
  icons: { icon: "/rustzip-icon.png" },
};

export default function RootLayout({ children }: Readonly<{ children: React.ReactNode }>) {
  return (
    <html lang="zh-CN">
      <body>
        <SiteHeader />
        <main>{children}</main>
        <SiteFooter />
      </body>
    </html>
  );
}

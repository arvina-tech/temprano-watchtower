import { defineConfig } from "vocs";

const siteTitle = "Tempo Watchtower";
const siteDescription =
  "Tempo Watchtower is a Rust service that accepts signed Tempo transactions, stores them durably, and broadcasts them throughout their validity window.";
const baseUrl = "https://docs.watchtower.temprano.io";
const ogImageUrl = `${baseUrl}/assets/og-cover.png`;

export default defineConfig({
  title: siteTitle,
  description: siteDescription,
  baseUrl,
  logoUrl: "/assets/logo-large.png",
  iconUrl: "/assets/icon.png",
  ogImageUrl: { "/": ogImageUrl },
  socials: [
    {
      icon: "github",
      label: "GitHub",
      link: "https://github.com/arvina-tech/tempo-watchtower",
    },
  ],
  sidebar: [
    { text: "Home", link: "/" },
    {
      text: "Getting Started",
      link: "/getting-started",
      items: [
        { text: "Installation", link: "/getting-started/installation" },
        { text: "Dependencies", link: "/getting-started/dependencies" },
        { text: "Configuration", link: "/getting-started/configuration" },
      ],
    },
    {
      text: "API Reference",
      link: "/api",
      items: [
        { text: "Common Types", link: "/api/common-types" },
        { text: "JSON-RPC", link: "/api/json-rpc" },
        { text: "Transactions", link: "/api/transactions" },
        { text: "Groups", link: "/api/groups" },
        { text: "Health", link: "/api/health" },
      ],
    },
    { text: "Concepts", link: "/concepts" },
    { text: "System Design", link: "/system-design" },
  ],
});

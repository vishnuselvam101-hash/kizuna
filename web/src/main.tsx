import "@fontsource/space-grotesk/latin-400.css";
import "@fontsource/space-grotesk/latin-500.css";
import "@fontsource/space-grotesk/latin-700.css";
import "@fontsource/jetbrains-mono/latin-400.css";
import "@fontsource/zen-kaku-gothic-new/japanese-400.css";
import "@fontsource/zen-kaku-gothic-new/japanese-700.css";
import "./theme.css";
import "./style.css";
import { createRoot } from "react-dom/client";
import { App } from "./App";

const root = createRoot(document.getElementById("root")!);
// The supplier portal is a separate entry for external supplier users (ADR 0023), loaded only on /portal.
if (window.location.pathname === "/portal" || window.location.pathname.startsWith("/portal/")) {
  import("./PortalApp").then(({ PortalApp }) => root.render(<PortalApp />));
} else {
  root.render(<App />);
}

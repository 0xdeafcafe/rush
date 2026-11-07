// The rush website, rush.forbes.red. TSX components render to plain HTML at
// build time; no framework is shipped to the browser.
import { defineConfig } from "astro/config";
import react from "@astrojs/react";

export default defineConfig({
  site: "https://rush.forbes.red",
  integrations: [react()],
  build: { inlineStylesheets: "auto" },
});

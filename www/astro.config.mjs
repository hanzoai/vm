import { defineConfig } from "astro/config";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
	site: "https://shuru.run",
	vite: {
		plugins: [tailwindcss()],
	},
	output: "static",
});

import { createApp } from "vue";

import App from "./App.vue";
import "./assets/main.css";

// Follow the system appearance (light or dark).
const dark = window.matchMedia("(prefers-color-scheme: dark)");
const applyTheme = () => document.documentElement.classList.toggle("dark", dark.matches);
applyTheme();
dark.addEventListener("change", applyTheme);

createApp(App).mount("#app");

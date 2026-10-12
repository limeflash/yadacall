import { mount } from "svelte";
import "./app.css";
import App from "./App.svelte";

const w = new URLSearchParams(location.search).get("w");
if (w === "tray" || w === "mini") document.documentElement.classList.add("popover");
document.body.classList.add("mg");

export default mount(App, { target: document.getElementById("app")! });

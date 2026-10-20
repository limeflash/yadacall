/** Tiny hash router: `#/meeting/<id>/transcript`, `#/settings`, … Works in the browser and in Tauri alike. */
export type Route =
  | { name: "meetings" }
  | { name: "meeting"; id: string; tab: "summary" | "transcript"; at?: number }
  | { name: "record" }
  | { name: "ask"; q?: string; folder?: string }
  | { name: "tasks" }
  | { name: "upload" }
  | { name: "settings" }
  | { name: "models" }
  | { name: "calendar" }
  | { name: "connections" }
  | { name: "google" }
  | { name: "onboarding" };

export function parse(hash: string): Route {
  const [path, query = ""] = hash.replace(/^#/, "").split("?");
  const parts = path.split("/").filter(Boolean);
  const qs = new URLSearchParams(query);
  switch (parts[0]) {
    case "meeting":
      return { name: "meeting", id: parts[1] ?? "", tab: parts[2] === "transcript" ? "transcript" : "summary", at: qs.has("at") ? Number(qs.get("at")) : undefined };
    case "record": return { name: "record" };
    case "ask": return { name: "ask", q: qs.get("q") ?? undefined, folder: qs.get("folder") ?? undefined };
    case "tasks": return { name: "tasks" };
    case "upload": return { name: "upload" };
    case "settings": return { name: "settings" };
    case "models": return { name: "models" };
    case "calendar": return { name: "calendar" };
    case "connections": return parts[1] === "google" ? { name: "google" } : { name: "connections" };
    case "onboarding": return { name: "onboarding" };
    default: return { name: "meetings" };
  }
}

export function href(r: Route): string {
  switch (r.name) {
    case "meeting": return `#/meeting/${r.id}/${r.tab}${r.at != null ? `?at=${Math.round(r.at)}` : ""}`;
    case "ask": {
      const qs = new URLSearchParams();
      if (r.q) qs.set("q", r.q);
      if (r.folder) qs.set("folder", r.folder);
      return qs.size ? `#/ask?${qs}` : "#/ask";
    }
    case "meetings": return "#/";
    case "google": return "#/connections/google";
    default: return `#/${r.name}`;
  }
}

export const nav = $state<{ route: Route; depth: number }>({ route: parse(typeof location === "undefined" ? "" : location.hash), depth: 0 });

if (typeof window !== "undefined") {
  window.addEventListener("hashchange", () => { nav.route = parse(location.hash); });
}

export function go(r: Route) {
  const h = href(r);
  if (location.hash !== h) { nav.depth++; location.hash = h; } else nav.route = r;
}
export function replace(r: Route) {
  history.replaceState(null, "", href(r));
  nav.route = r;
}
export function back(fallback: Route = { name: "meetings" }) {
  if (nav.depth > 0) { nav.depth--; history.back(); } else go(fallback);
}

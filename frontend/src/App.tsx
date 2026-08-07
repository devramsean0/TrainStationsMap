import { createSignal, For, onCleanup, onMount, Show } from "solid-js";
import L from "leaflet";
import "leaflet/dist/leaflet.css";

interface MarkerData {
  crs: string;
  lat: number;
  lng: number;
  colour: string;
  name: string;
  operator_name: string;
  status: string;
}

interface SyncStats {
  inserted: number;
  updated: number;
  crs_changed: number;
}

const STATUS_COLOURS: Record<string, string> = {
  unvisited: "#4080f0",
  visited: "#f0c040",
  done: "#40c040",
};

const STATUS_LABELS: Record<string, string> = {
  unvisited: "Unvisited",
  visited: "Visited",
  done: "Done",
};

const iconCache: Record<string, L.DivIcon> = {};

function getIcon(colour: string): L.DivIcon {
  if (!iconCache[colour]) {
    iconCache[colour] = L.divIcon({
      className: "",
      html: `<div style="
        width:12px;height:12px;
        background-color:${colour};
        border:2px solid rgba(0,0,0,0.55);
        border-radius:50%;
        box-shadow:0 1px 3px rgba(0,0,0,0.4);
      "></div>`,
      iconSize: [12, 12],
      iconAnchor: [6, 6],
      popupAnchor: [0, -8],
    });
  }
  return iconCache[colour];
}

// Auth state — module-level so buildPopup can read authed() at popup-open time.
const [authRequired, setAuthRequired] = createSignal(false);
const [authed, setAuthed] = createSignal(false);
const [authToken, setAuthToken] = createSignal<string | null>(null);

// buildPopup reads authed() at call time. Leaflet's lazy function binding means
// this is called fresh on every popup open, so auth changes are reflected.
function buildPopup(m: MarkerData): string {
  const buttons = authed()
    ? `<div style="display:flex;gap:4px">
        <button data-crs="${m.crs}" data-status="unvisited" style="
          flex:1;padding:5px 2px;border:none;border-radius:4px;cursor:pointer;
          background:${STATUS_COLOURS.unvisited};color:white;font-size:11px;font-weight:600;
          outline:${m.status === "unvisited" ? "2px solid rgba(0,0,0,0.5)" : "none"}">
          Unvisited
        </button>
        <button data-crs="${m.crs}" data-status="visited" style="
          flex:1;padding:5px 2px;border:none;border-radius:4px;cursor:pointer;
          background:${STATUS_COLOURS.visited};color:#333;font-size:11px;font-weight:600;
          outline:${m.status === "visited" ? "2px solid rgba(0,0,0,0.5)" : "none"}">
          Visited
        </button>
        <button data-crs="${m.crs}" data-status="done" style="
          flex:1;padding:5px 2px;border:none;border-radius:4px;cursor:pointer;
          background:${STATUS_COLOURS.done};color:white;font-size:11px;font-weight:600;
          outline:${m.status === "done" ? "2px solid rgba(0,0,0,0.5)" : "none"}">
          Done
        </button>
      </div>`
    : `<p style="color:#999;font-size:11px;margin:8px 0 0;text-align:center">
        Log in to update status
      </p>`;

  return `
    <div style="min-width:200px;font-family:sans-serif">
      <strong style="font-size:14px">${m.name}</strong>
      <div style="color:#666;font-size:12px;margin:2px 0 10px">
        ${m.operator_name} &middot; ${m.crs}
      </div>
      ${buttons}
    </div>
  `;
}

async function login() {
  const pw = window.prompt("Enter password:");
  if (!pw) return;
  const res = await fetch("/api/auth/login", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ password: pw }),
  });
  if (res.ok) {
    sessionStorage.setItem("authToken", pw);
    setAuthToken(pw);
    setAuthed(true);
  } else {
    alert("Incorrect password.");
  }
}

function logout() {
  sessionStorage.removeItem("authToken");
  setAuthToken(null);
  setAuthed(false);
}

export default function App() {
  let container: HTMLDivElement | undefined;
  let map: L.Map | undefined;
  const leafletMarkers = new Map<string, L.Marker>();

  const [stations, setStations] = createSignal<MarkerData[]>([]);
  const [search, setSearch] = createSignal("");
  const [syncing, setSyncing] = createSignal(false);
  const [syncResult, setSyncResult] = createSignal<string | null>(null);

  const filtered = () => {
    const q = search().toLowerCase().trim();
    if (!q) return stations();
    return stations().filter(
      (s) =>
        s.name.toLowerCase().includes(q) ||
        s.crs.toLowerCase().includes(q) ||
        s.operator_name.toLowerCase().includes(q),
    );
  };

  const counts = () => {
    const all = stations();
    const done = all.filter((s) => s.status === "done").length;
    const visited = all.filter((s) => s.status === "visited").length;
    return { total: all.length, done, visited, unvisited: all.length - done - visited };
  };

  async function applyStatus(crs: string, status: string): Promise<boolean> {
    const token = authToken();
    const headers: Record<string, string> = { "Content-Type": "application/json" };
    if (token) headers["Authorization"] = `Bearer ${token}`;

    const res = await fetch(`/api/stations/${crs}/status`, {
      method: "PATCH",
      headers,
      body: JSON.stringify({ status }),
    });

    if (res.status === 401) {
      setAuthed(false);
      alert("Session expired — please log in again.");
      return false;
    }

    if (res.ok || res.status === 204) {
      const colour = STATUS_COLOURS[status] ?? STATUS_COLOURS.unvisited;
      setStations((prev) =>
        prev.map((s) => {
          if (s.crs !== crs) return s;
          leafletMarkers.get(crs)?.setIcon(getIcon(colour));
          return { ...s, status, colour };
        }),
      );
      return true;
    }
    return false;
  }

  // Attach a Leaflet marker for one station and register it in leafletMarkers.
  function addMarker(m: MarkerData) {
    const marker = L.marker([m.lat, m.lng], { icon: getIcon(m.colour) });

    marker.bindPopup(() => {
      const current = stations().find((s) => s.crs === m.crs) ?? m;
      return buildPopup(current);
    });

    marker.on("popupopen", () => {
      const popupEl = marker.getPopup()!.getElement()!;
      popupEl
        .querySelectorAll<HTMLButtonElement>("[data-crs][data-status]")
        .forEach((btn) => {
          btn.addEventListener("click", async () => {
            const success = await applyStatus(btn.dataset.crs!, btn.dataset.status!);
            if (success) marker.closePopup();
          });
        });
    });

    marker.addTo(map!);
    leafletMarkers.set(m.crs, marker);
  }

  // Tear down all markers and rebuild from /api/markers.
  async function reloadMarkers() {
    leafletMarkers.forEach((m) => m.remove());
    leafletMarkers.clear();
    const data: MarkerData[] = await fetch("/api/markers").then((r) => r.json());
    setStations(data);
    for (const m of data) addMarker(m);
  }

  async function triggerSync() {
    setSyncing(true);
    setSyncResult(null);

    const token = authToken();
    const headers: Record<string, string> = {};
    if (token) headers["Authorization"] = `Bearer ${token}`;

    try {
      const res = await fetch("/api/stations/refresh", { method: "POST", headers });
      if (res.ok) {
        const s = (await res.json()) as SyncStats;
        setSyncResult(
          `Sync complete — ${s.inserted} new, ${s.updated} updated, ${s.crs_changed} renamed`,
        );
        await reloadMarkers();
      } else if (res.status === 503) {
        setSyncResult("Server error: RDM API keys not configured");
      } else {
        setSyncResult(`Sync failed (${res.status})`);
      }
    } catch {
      setSyncResult("Network error during sync");
    } finally {
      setSyncing(false);
      setTimeout(() => setSyncResult(null), 8000);
    }
  }

  function flyToStation(s: MarkerData) {
    if (!map) return;
    map.once("moveend", () => leafletMarkers.get(s.crs)?.openPopup());
    map.flyTo([s.lat, s.lng], 14);
  }

  onMount(async () => {
    // Determine whether auth is required, then validate any stored token.
    const noCredRes = await fetch("/api/auth/verify");
    if (noCredRes.ok) {
      setAuthRequired(false);
      setAuthed(true);
    } else {
      setAuthRequired(true);
      const stored = sessionStorage.getItem("authToken");
      if (stored) {
        const tokenRes = await fetch("/api/auth/verify", {
          headers: { Authorization: `Bearer ${stored}` },
        });
        if (tokenRes.ok) {
          setAuthToken(stored);
          setAuthed(true);
        } else {
          sessionStorage.removeItem("authToken");
        }
      }
    }

    map = L.map(container!).setView([54.5, -3.5], 6);

    L.tileLayer("/api/tiles/{z}/{x}/{y}", {
      attribution:
        '&copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors',
      maxZoom: 19,
    }).addTo(map);

    const data: MarkerData[] = await fetch("/api/markers").then((r) => r.json());
    setStations(data);
    for (const m of data) addMarker(m);
  });

  onCleanup(() => map?.remove());

  return (
    <>
      <div ref={container} style={{ position: "fixed", inset: "0" }} />

      {/* Side panel */}
      <div
        style={{
          position: "fixed",
          top: "10px",
          right: "10px",
          width: "300px",
          "max-height": "calc(100vh - 20px)",
          background: "white",
          "border-radius": "8px",
          "box-shadow": "0 2px 16px rgba(0,0,0,0.35)",
          display: "flex",
          "flex-direction": "column",
          "z-index": "1000",
          overflow: "hidden",
          "font-family": "sans-serif",
        }}
      >
        {/* Header */}
        <div style={{ padding: "12px 14px 8px", "border-bottom": "1px solid #eee" }}>
          <div
            style={{
              display: "flex",
              "justify-content": "space-between",
              "align-items": "center",
              "margin-bottom": "6px",
            }}
          >
            <strong style={{ "font-size": "14px" }}>Stations</strong>
            <div style={{ display: "flex", "align-items": "center", gap: "8px" }}>
              <span style={{ "font-size": "11px", color: "#666" }}>{counts().total} total</span>
              <Show when={authed()}>
                <button
                  onClick={triggerSync}
                  disabled={syncing()}
                  style={{
                    padding: "3px 10px",
                    background: syncing() ? "#ccc" : "#2563eb",
                    color: "white",
                    border: "none",
                    "border-radius": "4px",
                    cursor: syncing() ? "default" : "pointer",
                    "font-size": "11px",
                    "font-weight": "600",
                    "font-family": "sans-serif",
                  }}
                >
                  {syncing() ? "Syncing…" : "Sync"}
                </button>
              </Show>
            </div>
          </div>

          {/* Sync result banner */}
          <Show when={syncResult()}>
            <div
              style={{
                "font-size": "11px",
                color: "#166534",
                background: "#dcfce7",
                border: "1px solid #86efac",
                "border-radius": "4px",
                padding: "4px 8px",
                "margin-bottom": "6px",
              }}
            >
              {syncResult()}
            </div>
          </Show>

          <div style={{ display: "flex", gap: "6px", "margin-bottom": "8px" }}>
            {(
              [
                { key: "done", label: `${counts().done} done`, fg: "white" },
                { key: "visited", label: `${counts().visited} visited`, fg: "#333" },
                { key: "unvisited", label: `${counts().unvisited} unvisited`, fg: "white" },
              ] as const
            ).map(({ key, label, fg }) => (
              <span
                style={{
                  "font-size": "11px",
                  background: STATUS_COLOURS[key],
                  color: fg,
                  padding: "2px 7px",
                  "border-radius": "10px",
                  "white-space": "nowrap",
                }}
              >
                {label}
              </span>
            ))}
          </div>

          <input
            type="text"
            placeholder="Search by name, CRS or operator…"
            value={search()}
            onInput={(e) => setSearch(e.currentTarget.value)}
            style={{
              width: "100%",
              "box-sizing": "border-box",
              padding: "6px 10px",
              border: "1px solid #ddd",
              "border-radius": "5px",
              "font-size": "13px",
              outline: "none",
            }}
          />
          <div style={{ "font-size": "11px", color: "#999", "margin-top": "4px" }}>
            {filtered().length === stations().length
              ? `${stations().length} stations`
              : `${filtered().length} of ${stations().length}`}
          </div>
        </div>

        {/* Station list */}
        <div style={{ "overflow-y": "auto", flex: "1" }}>
          <For each={filtered()}>
            {(s) => (
              <div style={{ padding: "8px 14px", "border-bottom": "1px solid #f0f0f0" }}>
                <div
                  style={{
                    display: "flex",
                    "align-items": "center",
                    gap: "7px",
                    cursor: "pointer",
                    "margin-bottom": "2px",
                  }}
                  onClick={() => flyToStation(s)}
                  onMouseEnter={(e) =>
                    ((e.currentTarget as HTMLDivElement).style.opacity = "0.75")
                  }
                  onMouseLeave={(e) =>
                    ((e.currentTarget as HTMLDivElement).style.opacity = "1")
                  }
                >
                  <span
                    style={{
                      width: "10px",
                      height: "10px",
                      "border-radius": "50%",
                      "background-color": s.colour,
                      border: "1.5px solid rgba(0,0,0,0.3)",
                      "flex-shrink": "0",
                      display: "inline-block",
                    }}
                  />
                  <span style={{ "font-size": "13px", "font-weight": "600", flex: "1" }}>
                    {s.name}
                  </span>
                </div>

                <div
                  style={{
                    "font-size": "11px",
                    color: "#888",
                    "padding-left": "17px",
                    "margin-bottom": authed() ? "6px" : "0",
                  }}
                >
                  {s.operator_name} · {s.crs}
                </div>

                <Show when={authed()}>
                  <div style={{ display: "flex", gap: "4px", "padding-left": "17px" }}>
                    {(["unvisited", "visited", "done"] as const).map((st) => (
                      <button
                        onClick={() => applyStatus(s.crs, st)}
                        style={{
                          flex: "1",
                          padding: "3px 2px",
                          border:
                            s.status === st
                              ? "2px solid rgba(0,0,0,0.35)"
                              : "2px solid transparent",
                          "border-radius": "4px",
                          cursor: "pointer",
                          background: STATUS_COLOURS[st],
                          color: st === "visited" ? "#333" : "white",
                          "font-size": "10px",
                          "font-weight": "600",
                          opacity: s.status === st ? "1" : "0.45",
                          transition: "opacity 0.15s",
                        }}
                      >
                        {STATUS_LABELS[st]}
                      </button>
                    ))}
                  </div>
                </Show>
              </div>
            )}
          </For>
        </div>
      </div>

      {/* Login / logout — only shown when a password is configured */}
      <Show when={authRequired()}>
        <div
          style={{
            position: "fixed",
            bottom: "16px",
            left: "50%",
            transform: "translateX(-50%)",
            "z-index": "1000",
          }}
        >
          <Show
            when={authed()}
            fallback={
              <button
                onClick={login}
                style={{
                  padding: "9px 22px",
                  background: "#2563eb",
                  color: "white",
                  border: "none",
                  "border-radius": "20px",
                  cursor: "pointer",
                  "font-size": "13px",
                  "font-weight": "600",
                  "font-family": "sans-serif",
                  "box-shadow": "0 2px 10px rgba(0,0,0,0.3)",
                }}
              >
                Sign In
              </button>
            }
          >
            <button
              onClick={logout}
              style={{
                padding: "9px 22px",
                background: "rgba(255,255,255,0.92)",
                color: "#444",
                border: "1px solid #ccc",
                "border-radius": "20px",
                cursor: "pointer",
                "font-size": "13px",
                "font-weight": "500",
                "font-family": "sans-serif",
                "box-shadow": "0 2px 8px rgba(0,0,0,0.15)",
              }}
            >
              Sign Out
            </button>
          </Show>
        </div>
      </Show>
    </>
  );
}

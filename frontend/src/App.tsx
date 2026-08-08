import { createSignal, For, onCleanup, onMount, Show } from "solid-js";
import L from "leaflet";
import "leaflet/dist/leaflet.css";

interface Label {
  id: number;
  name: string;
  colour: string;
}

interface MarkerData {
  crs: string;
  lat: number;
  lng: number;
  colour: string;
  name: string;
  operator_name: string;
  status: string;
  labels: number[];
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
  const [labels, setLabels] = createSignal<Label[]>([]);
  const [search, setSearch] = createSignal("");
  const [selectedLabel, setSelectedLabel] = createSignal<number | null>(null);
  const [syncing, setSyncing] = createSignal(false);
  const [syncResult, setSyncResult] = createSignal<string | null>(null);

  // Label form state
  const [showLabelForm, setShowLabelForm] = createSignal(false);
  const [editingLabelId, setEditingLabelId] = createSignal<number | null>(null);
  const [formName, setFormName] = createSignal("");
  const [formColour, setFormColour] = createSignal("#3b82f6");

  const filtered = () => {
    const q = search().toLowerCase().trim();
    const labelId = selectedLabel();
    let result = stations();
    if (q)
      result = result.filter(
        (s) =>
          s.name.toLowerCase().includes(q) ||
          s.crs.toLowerCase().includes(q) ||
          s.operator_name.toLowerCase().includes(q),
      );
    if (labelId !== null) result = result.filter((s) => s.labels.includes(labelId));
    return result;
  };

  const counts = () => {
    const all = stations();
    const done = all.filter((s) => s.status === "done").length;
    const visited = all.filter((s) => s.status === "visited").length;
    return { total: all.length, done, visited, unvisited: all.length - done - visited };
  };

  const labelStationCount = (labelId: number) =>
    stations().filter((s) => s.labels.includes(labelId)).length;

  // "done" takes priority; otherwise first label colour; otherwise status colour.
  function computeColour(s: { status: string; labels: number[] }): string {
    if (s.status === "done") return STATUS_COLOURS.done;
    if (s.labels.length > 0) {
      const label = labels().find((l) => l.id === s.labels[0]);
      if (label) return label.colour;
    }
    return STATUS_COLOURS[s.status] ?? STATUS_COLOURS.unvisited;
  }

  function authHeaders(): Record<string, string> {
    const token = authToken();
    const h: Record<string, string> = { "Content-Type": "application/json" };
    if (token) h["Authorization"] = `Bearer ${token}`;
    return h;
  }

  async function applyStatus(crs: string, status: string): Promise<boolean> {
    const res = await fetch(`/api/stations/${crs}/status`, {
      method: "PATCH",
      headers: authHeaders(),
      body: JSON.stringify({ status }),
    });

    if (res.status === 401) {
      setAuthed(false);
      alert("Session expired — please log in again.");
      return false;
    }

    if (res.ok || res.status === 204) {
      setStations((prev) =>
        prev.map((s) => {
          if (s.crs !== crs) return s;
          const updated = { ...s, status };
          const colour = computeColour(updated);
          leafletMarkers.get(crs)?.setIcon(getIcon(colour));
          return { ...updated, colour };
        }),
      );
      return true;
    }
    return false;
  }

  async function toggleStationLabel(crs: string, labelId: number, assigned: boolean) {
    if (assigned) {
      await fetch(`/api/stations/${crs}/labels/${labelId}`, {
        method: "DELETE",
        headers: authHeaders(),
      });
      setStations((prev) =>
        prev.map((s) => {
          if (s.crs !== crs) return s;
          const updated = { ...s, labels: s.labels.filter((id) => id !== labelId) };
          const colour = computeColour(updated);
          leafletMarkers.get(crs)?.setIcon(getIcon(colour));
          return { ...updated, colour };
        }),
      );
    } else {
      await fetch(`/api/stations/${crs}/labels`, {
        method: "POST",
        headers: authHeaders(),
        body: JSON.stringify({ label_id: labelId }),
      });
      setStations((prev) =>
        prev.map((s) => {
          if (s.crs !== crs) return s;
          const updated = { ...s, labels: [...s.labels, labelId] };
          const colour = computeColour(updated);
          leafletMarkers.get(crs)?.setIcon(getIcon(colour));
          return { ...updated, colour };
        }),
      );
    }
  }

  async function saveLabel() {
    const name = formName().trim();
    if (!name) return;
    const editId = editingLabelId();

    if (editId !== null) {
      const res = await fetch(`/api/labels/${editId}`, {
        method: "PUT",
        headers: authHeaders(),
        body: JSON.stringify({ name, colour: formColour() }),
      });
      if (res.ok || res.status === 204) {
        setLabels((prev) =>
          prev.map((l) => (l.id === editId ? { ...l, name, colour: formColour() } : l)),
        );
        // Recompute colours for stations that use this label
        setStations((prev) =>
          prev.map((s) => {
            if (!s.labels.includes(editId)) return s;
            const colour = computeColour(s);
            leafletMarkers.get(s.crs)?.setIcon(getIcon(colour));
            return { ...s, colour };
          }),
        );
      }
    } else {
      const res = await fetch("/api/labels", {
        method: "POST",
        headers: authHeaders(),
        body: JSON.stringify({ name, colour: formColour() }),
      });
      if (res.ok) {
        const created: Label = await res.json();
        setLabels((prev) => [...prev, created].sort((a, b) => a.name.localeCompare(b.name)));
      }
    }

    setShowLabelForm(false);
    setEditingLabelId(null);
    setFormName("");
    setFormColour("#3b82f6");
  }

  function startEditLabel(label: Label) {
    setEditingLabelId(label.id);
    setFormName(label.name);
    setFormColour(label.colour);
    setShowLabelForm(true);
  }

  function cancelLabelForm() {
    setShowLabelForm(false);
    setEditingLabelId(null);
    setFormName("");
    setFormColour("#3b82f6");
  }

  async function removeLabel(id: number) {
    const res = await fetch(`/api/labels/${id}`, {
      method: "DELETE",
      headers: authHeaders(),
    });
    if (res.ok || res.status === 204) {
      setLabels((prev) => prev.filter((l) => l.id !== id));
      setStations((prev) =>
        prev.map((s) => {
          if (!s.labels.includes(id)) return s;
          const updated = { ...s, labels: s.labels.filter((lid) => lid !== id) };
          const colour = computeColour(updated);
          leafletMarkers.get(s.crs)?.setIcon(getIcon(colour));
          return { ...updated, colour };
        }),
      );
      if (selectedLabel() === id) setSelectedLabel(null);
    }
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
    const withColour = data.map((s) => ({ ...s, colour: computeColour(s) }));
    setStations(withColour);
    for (const m of withColour) addMarker(m);
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

    const [markersData, labelsData] = await Promise.all([
      fetch("/api/markers").then((r) => r.json()) as Promise<MarkerData[]>,
      fetch("/api/labels").then((r) => r.json()) as Promise<Label[]>,
    ]);

    // Set labels first so computeColour has the data it needs.
    setLabels(labelsData);
    const stationsWithColour = markersData.map((s) => ({
      ...s,
      colour: computeColour(s),
    }));
    setStations(stationsWithColour);
    for (const m of stationsWithColour) addMarker(m);
  });

  onCleanup(() => map?.remove());

  const panelStyle = {
    position: "fixed" as const,
    top: "10px",
    "max-height": "calc(100vh - 20px)",
    background: "white",
    "border-radius": "8px",
    "box-shadow": "0 2px 16px rgba(0,0,0,0.35)",
    display: "flex",
    "flex-direction": "column" as const,
    "z-index": "1000",
    overflow: "hidden",
    "font-family": "sans-serif",
  };

  return (
    <>
      <div ref={container} style={{ position: "fixed", inset: "0" }} />

      {/* Labels pane — left */}
      <div style={{ ...panelStyle, left: "10px", width: "240px" }}>
        <div style={{ padding: "12px 14px 8px", "border-bottom": "1px solid #eee" }}>
          <div
            style={{
              display: "flex",
              "justify-content": "space-between",
              "align-items": "center",
              "margin-bottom": showLabelForm() ? "10px" : "0",
            }}
          >
            <strong style={{ "font-size": "14px" }}>Labels</strong>
            <Show when={authed()}>
              <button
                onClick={() => {
                  setEditingLabelId(null);
                  setFormName("");
                  setFormColour("#3b82f6");
                  setShowLabelForm(true);
                }}
                style={{
                  padding: "3px 10px",
                  background: "#2563eb",
                  color: "white",
                  border: "none",
                  "border-radius": "4px",
                  cursor: "pointer",
                  "font-size": "11px",
                  "font-weight": "600",
                  "font-family": "sans-serif",
                }}
              >
                + New
              </button>
            </Show>
          </div>

          {/* Label form */}
          <Show when={showLabelForm()}>
            <div
              style={{
                background: "#f8fafc",
                border: "1px solid #e2e8f0",
                "border-radius": "6px",
                padding: "10px",
                display: "flex",
                "flex-direction": "column",
                gap: "8px",
              }}
            >
              <input
                type="text"
                placeholder="Label name"
                value={formName()}
                onInput={(e) => setFormName(e.currentTarget.value)}
                style={{
                  width: "100%",
                  "box-sizing": "border-box",
                  padding: "5px 8px",
                  border: "1px solid #ddd",
                  "border-radius": "4px",
                  "font-size": "12px",
                  outline: "none",
                }}
              />
              <div style={{ display: "flex", "align-items": "center", gap: "8px" }}>
                <label style={{ "font-size": "11px", color: "#666" }}>Colour</label>
                <input
                  type="color"
                  value={formColour()}
                  onInput={(e) => setFormColour(e.currentTarget.value)}
                  style={{
                    width: "36px",
                    height: "24px",
                    border: "1px solid #ddd",
                    "border-radius": "4px",
                    cursor: "pointer",
                    padding: "1px",
                  }}
                />
                <span
                  style={{
                    "font-size": "11px",
                    color: "#666",
                    flex: "1",
                    "font-family": "monospace",
                  }}
                >
                  {formColour()}
                </span>
              </div>
              <div style={{ display: "flex", gap: "6px" }}>
                <button
                  onClick={saveLabel}
                  style={{
                    flex: "1",
                    padding: "5px",
                    background: "#2563eb",
                    color: "white",
                    border: "none",
                    "border-radius": "4px",
                    cursor: "pointer",
                    "font-size": "11px",
                    "font-weight": "600",
                    "font-family": "sans-serif",
                  }}
                >
                  {editingLabelId() !== null ? "Update" : "Create"}
                </button>
                <button
                  onClick={cancelLabelForm}
                  style={{
                    flex: "1",
                    padding: "5px",
                    background: "#f1f5f9",
                    color: "#444",
                    border: "1px solid #e2e8f0",
                    "border-radius": "4px",
                    cursor: "pointer",
                    "font-size": "11px",
                    "font-family": "sans-serif",
                  }}
                >
                  Cancel
                </button>
              </div>
            </div>
          </Show>
        </div>

        {/* Label list */}
        <div style={{ "overflow-y": "auto", flex: "1" }}>
          <Show
            when={labels().length > 0}
            fallback={
              <div
                style={{
                  padding: "16px 14px",
                  "font-size": "12px",
                  color: "#aaa",
                  "text-align": "center",
                }}
              >
                No labels yet
              </div>
            }
          >
            <Show when={selectedLabel() !== null}>
              <div
                style={{
                  padding: "6px 14px",
                  "border-bottom": "1px solid #f0f0f0",
                }}
              >
                <button
                  onClick={() => setSelectedLabel(null)}
                  style={{
                    width: "100%",
                    padding: "4px 8px",
                    background: "#f1f5f9",
                    border: "1px solid #e2e8f0",
                    "border-radius": "4px",
                    cursor: "pointer",
                    "font-size": "11px",
                    color: "#555",
                    "font-family": "sans-serif",
                  }}
                >
                  × Show all stations
                </button>
              </div>
            </Show>
            <For each={labels()}>
              {(label) => {
                const isSelected = () => selectedLabel() === label.id;
                const count = () => labelStationCount(label.id);
                return (
                  <div
                    style={{
                      padding: "8px 14px",
                      "border-bottom": "1px solid #f0f0f0",
                      background: isSelected() ? "#eff6ff" : "transparent",
                      cursor: "pointer",
                    }}
                    onClick={() =>
                      setSelectedLabel(isSelected() ? null : label.id)
                    }
                  >
                    <div
                      style={{
                        display: "flex",
                        "align-items": "center",
                        gap: "8px",
                      }}
                    >
                      <span
                        style={{
                          width: "12px",
                          height: "12px",
                          "border-radius": "50%",
                          "background-color": label.colour,
                          border: "1.5px solid rgba(0,0,0,0.2)",
                          "flex-shrink": "0",
                          display: "inline-block",
                        }}
                      />
                      <span
                        style={{
                          "font-size": "13px",
                          "font-weight": isSelected() ? "700" : "500",
                          flex: "1",
                        }}
                      >
                        {label.name}
                      </span>
                      <span
                        style={{
                          "font-size": "10px",
                          color: "#888",
                          background: "#f0f0f0",
                          padding: "1px 5px",
                          "border-radius": "8px",
                        }}
                      >
                        {count()}
                      </span>
                      <Show when={authed()}>
                        <button
                          onClick={(e) => {
                            e.stopPropagation();
                            startEditLabel(label);
                          }}
                          style={{
                            background: "none",
                            border: "none",
                            cursor: "pointer",
                            color: "#888",
                            "font-size": "12px",
                            padding: "0 2px",
                            "line-height": "1",
                          }}
                          title="Edit label"
                        >
                          ✎
                        </button>
                        <button
                          onClick={(e) => {
                            e.stopPropagation();
                            removeLabel(label.id);
                          }}
                          style={{
                            background: "none",
                            border: "none",
                            cursor: "pointer",
                            color: "#e05",
                            "font-size": "13px",
                            padding: "0 2px",
                            "line-height": "1",
                          }}
                          title="Delete label"
                        >
                          ×
                        </button>
                      </Show>
                    </div>
                  </div>
                );
              }}
            </For>
          </Show>
        </div>
      </div>

      {/* Stations pane — right */}
      <div style={{ ...panelStyle, right: "10px", width: "300px" }}>
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
                    "margin-bottom": "4px",
                  }}
                >
                  {s.operator_name} · {s.crs}
                </div>

                {/* Labels: toggles when authed, read-only pills otherwise */}
                <Show when={labels().length > 0}>
                  <div
                    style={{
                      display: "flex",
                      gap: "4px",
                      "padding-left": "17px",
                      "flex-wrap": "wrap",
                      "margin-bottom": "4px",
                    }}
                  >
                    <For each={labels()}>
                      {(label) => {
                        const assigned = () => s.labels.includes(label.id);
                        return (
                          <Show when={authed() || assigned()}>
                            <button
                              onClick={() => authed() && toggleStationLabel(s.crs, label.id, assigned())}
                              style={{
                                display: "inline-flex",
                                "align-items": "center",
                                gap: "3px",
                                padding: "2px 6px",
                                border: assigned()
                                  ? `1.5px solid ${label.colour}`
                                  : "1.5px solid #ddd",
                                "border-radius": "8px",
                                cursor: authed() ? "pointer" : "default",
                                background: assigned() ? label.colour + "22" : "transparent",
                                "font-size": "10px",
                                color: "#333",
                                opacity: assigned() ? "1" : "0.4",
                                transition: "opacity 0.15s",
                                "font-family": "sans-serif",
                              }}
                              title={
                                authed()
                                  ? assigned()
                                    ? `Remove "${label.name}"`
                                    : `Add "${label.name}"`
                                  : label.name
                              }
                            >
                              <span
                                style={{
                                  width: "7px",
                                  height: "7px",
                                  "border-radius": "50%",
                                  "background-color": label.colour,
                                  display: "inline-block",
                                  "flex-shrink": "0",
                                }}
                              />
                              {label.name}
                            </button>
                          </Show>
                        );
                      }}
                    </For>
                  </div>
                </Show>

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

import { onCleanup, onMount } from "solid-js";
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

const STATUS_COLOURS: Record<string, string> = {
  unvisited: "#4080f0",
  visited: "#f0c040",
  done: "#40c040",
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

function buildPopup(m: MarkerData): string {
  return `
    <div style="min-width:200px;font-family:sans-serif">
      <strong style="font-size:14px">${m.name}</strong>
      <div style="color:#666;font-size:12px;margin:2px 0 10px">
        ${m.operator_name} &middot; ${m.crs}
      </div>
      <div style="display:flex;gap:4px">
        <button data-crs="${m.crs}" data-status="unvisited" style="
          flex:1;padding:5px 2px;border:none;border-radius:4px;cursor:pointer;
          background:${STATUS_COLOURS.unvisited};color:white;font-size:11px;font-weight:600">
          Unvisited
        </button>
        <button data-crs="${m.crs}" data-status="visited" style="
          flex:1;padding:5px 2px;border:none;border-radius:4px;cursor:pointer;
          background:${STATUS_COLOURS.visited};color:#333;font-size:11px;font-weight:600">
          Visited
        </button>
        <button data-crs="${m.crs}" data-status="done" style="
          flex:1;padding:5px 2px;border:none;border-radius:4px;cursor:pointer;
          background:${STATUS_COLOURS.done};color:white;font-size:11px;font-weight:600">
          Done
        </button>
      </div>
    </div>
  `;
}

export default function App() {
  let container: HTMLDivElement | undefined;
  let map: L.Map | undefined;

  onMount(async () => {
    map = L.map(container!).setView([54.5, -3.5], 6);

    L.tileLayer("/api/tiles/{z}/{x}/{y}", {
      attribution:
        '&copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors',
      maxZoom: 19,
    }).addTo(map);

    const leafletMarkers = new Map<string, L.Marker>();

    const data: MarkerData[] = await fetch("/api/markers").then((r) => r.json());

    for (const m of data) {
      const marker = L.marker([m.lat, m.lng], { icon: getIcon(m.colour) });

      marker.bindPopup(buildPopup(m));

      marker.on("popupopen", () => {
        const popupEl = marker.getPopup()!.getElement()!;
        popupEl
          .querySelectorAll<HTMLButtonElement>("[data-crs][data-status]")
          .forEach((btn) => {
            btn.addEventListener("click", async () => {
              const crs = btn.dataset.crs!;
              const status = btn.dataset.status!;
              await fetch(`/api/stations/${crs}/status`, {
                method: "PATCH",
                headers: { "Content-Type": "application/json" },
                body: JSON.stringify({ status }),
              });
              const colour = STATUS_COLOURS[status] ?? STATUS_COLOURS.unvisited;
              leafletMarkers.get(crs)?.setIcon(getIcon(colour));
              marker.closePopup();
            });
          });
      });

      marker.addTo(map!);
      leafletMarkers.set(m.crs, marker);
    }
  });

  onCleanup(() => map?.remove());

  return <div ref={container} style={{ position: "fixed", inset: "0" }} />;
}

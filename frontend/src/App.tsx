import { onCleanup, onMount } from "solid-js";
import L from "leaflet";
import "leaflet/dist/leaflet.css";

interface Marker {
  lat: number;
  lng: number;
  colour: string;
  label?: string;
}

const UK_BOUNDS = L.latLngBounds(L.latLng(49.8, -8.7), L.latLng(60.9, 1.9));

const colourIcons: Record<string, L.DivIcon> = {};

function getColourIcon(colour: string): L.DivIcon {
  if (!colourIcons[colour]) {
    colourIcons[colour] = L.divIcon({
      className: "",
      html: `<div style="
        width:16px;height:16px;
        background-color:${colour};
        border:2px solid rgba(0,0,0,0.5);
        border-radius:50%;
        box-shadow:0 1px 3px rgba(0,0,0,0.4);
      "></div>`,
      iconSize: [16, 16],
      iconAnchor: [8, 8],
      popupAnchor: [0, -10],
    });
  }
  return colourIcons[colour];
}

export default function App() {
  let container: HTMLDivElement | undefined;
  let map: L.Map | undefined;

  onMount(async () => {
    map = L.map(container!, {
      maxBounds: UK_BOUNDS,
      maxBoundsViscosity: 1.0,
      minZoom: 5,
      maxZoom: 19,
    }).setView([54.5, -3.5], 6);

    L.tileLayer("/api/tiles/{z}/{x}/{y}", {
      attribution:
        '&copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors',
      minZoom: 5,
      maxZoom: 19,
      bounds: UK_BOUNDS,
    }).addTo(map);

    const markers: Marker[] = await fetch("/api/markers").then((r) => r.json());
    for (const m of markers) {
      const marker = L.marker([m.lat, m.lng], { icon: getColourIcon(m.colour) });
      if (m.label) marker.bindPopup(m.label);
      marker.addTo(map!);
    }
  });

  onCleanup(() => map?.remove());

  return <div ref={container} style={{ width: "100vw", height: "100vh" }} />;
}

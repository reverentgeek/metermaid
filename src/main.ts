import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

interface DeviceInfo {
  name: string;
  isDefault: boolean;
}

interface DeviceConfig {
  channels: number;
  defaultSampleRate: number;
  sampleRates: number[];
}

interface StreamInfo {
  deviceName: string;
  sampleRate: number;
  channels: number;
}

interface Metrics {
  momentary: number;
  shortTerm: number;
  integrated: number;
  lra: number;
  truePeakDb: number;
  truePeakMaxDb: number;
  spectrum: number[];
  sampleRate: number;
  channels: number;
}

const LOUDNESS_FLOOR = -70;
const PEAK_FLOOR = -120;
const SPECTRUM_FLOOR = -90;
const SPECTRUM_TOP = 0;
const PEAK_RELEASE_DB = 2; // live true-peak meter fall per update

let running = false;
let latest: Metrics | null = null;
let peaks: number[] = []; // smoothed spectrum peak-hold per band
let displayedPeak = PEAK_FLOOR; // live true-peak with release ballistics
let clipLatched = false;

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const deviceSelect = $<HTMLSelectElement>("device");
const channelSelect = $<HTMLSelectElement>("channels");
const rateSelect = $<HTMLSelectElement>("rate");
const toggleBtn = $<HTMLButtonElement>("toggle");
const resetBtn = $<HTMLButtonElement>("reset");
const statusEl = $<HTMLSpanElement>("status");
const targetInput = $<HTMLInputElement>("target");
const ceilingInput = $<HTMLInputElement>("ceiling");
const deltaEl = $<HTMLDivElement>("delta");
const tpCard = $<HTMLDivElement>("tpCard");
const clipFlag = $<HTMLSpanElement>("clipFlag");
const canvas = $<HTMLCanvasElement>("spectrum");
const ctx = canvas.getContext("2d")!;

function fmt(v: number, floor = LOUDNESS_FLOOR): string {
  if (!Number.isFinite(v) || v <= floor) return "−∞";
  return v.toFixed(1);
}

function setStatus(text: string, kind: "ok" | "err" | "idle") {
  statusEl.textContent = text;
  statusEl.className = `status status-${kind}`;
}

function configControlsEnabled(enabled: boolean) {
  deviceSelect.disabled = !enabled;
  channelSelect.disabled = !enabled;
  rateSelect.disabled = !enabled;
}

async function loadDevices() {
  try {
    const devices = await invoke<DeviceInfo[]>("list_devices");
    deviceSelect.innerHTML = "";
    if (devices.length === 0) {
      const opt = document.createElement("option");
      opt.textContent = "No input devices found";
      opt.disabled = true;
      deviceSelect.append(opt);
      return;
    }
    for (const d of devices) {
      const opt = document.createElement("option");
      opt.value = d.name;
      opt.textContent = d.isDefault ? `${d.name} (default)` : d.name;
      if (d.isDefault) opt.selected = true;
      deviceSelect.append(opt);
    }
    await refreshDeviceConfig();
  } catch (e) {
    setStatus(String(e), "err");
  }
}

// Build generic channel options: stereo pairs first, then mono channels.
function populateChannels(count: number) {
  channelSelect.innerHTML = "";
  const add = (label: string, indices: number[]) => {
    const opt = document.createElement("option");
    opt.value = indices.join(",");
    opt.textContent = label;
    channelSelect.append(opt);
  };
  for (let i = 0; i + 1 < count; i += 2) {
    add(`Ch ${i + 1}–${i + 2}`, [i, i + 1]);
  }
  for (let i = 0; i < count; i++) {
    add(`Ch ${i + 1} (mono)`, [i]);
  }
  if (count === 0) add("No channels", []);
  channelSelect.selectedIndex = 0; // first stereo pair (Ch 1–2) when available
}

function populateRates(rates: number[], def: number) {
  rateSelect.innerHTML = "";
  for (const r of rates) {
    const opt = document.createElement("option");
    opt.value = String(r);
    opt.textContent = `${r / 1000} kHz`;
    if (r === def) opt.selected = true;
    rateSelect.append(opt);
  }
}

async function refreshDeviceConfig() {
  try {
    const cfg = await invoke<DeviceConfig>("get_device_config", {
      device: deviceSelect.value || null,
    });
    populateChannels(cfg.channels);
    populateRates(cfg.sampleRates, cfg.defaultSampleRate);
  } catch (e) {
    setStatus(String(e), "err");
  }
}

async function start() {
  try {
    const channels = channelSelect.value
      ? channelSelect.value.split(",").map(Number)
      : [];
    const sampleRate = rateSelect.value ? Number(rateSelect.value) : null;
    const info = await invoke<StreamInfo>("start_capture", {
      device: deviceSelect.value || null,
      sampleRate,
      channels,
    });
    running = true;
    clipLatched = false;
    displayedPeak = PEAK_FLOOR;
    toggleBtn.textContent = "Stop";
    toggleBtn.classList.add("running");
    configControlsEnabled(false);
    const mode = info.channels === 1 ? "mono" : `${info.channels} ch`;
    setStatus(`${info.sampleRate / 1000} kHz · ${mode}`, "ok");
  } catch (e) {
    setStatus(String(e), "err");
  }
}

async function stop() {
  try {
    await invoke("stop_capture");
  } catch (e) {
    setStatus(String(e), "err");
  }
  running = false;
  latest = null;
  toggleBtn.textContent = "Start";
  toggleBtn.classList.remove("running");
  configControlsEnabled(true);
  setStatus("stopped", "idle");
}

function updateReadouts(m: Metrics) {
  $("integrated").textContent = fmt(m.integrated);
  $("shortTerm").textContent = fmt(m.shortTerm);
  $("momentary").textContent = fmt(m.momentary);
  $("lra").textContent = m.lra > 0 ? m.lra.toFixed(1) : "0.0";

  // Live true peak with release ballistics; held max from the engine.
  const live = m.truePeakDb;
  displayedPeak = live > displayedPeak ? live : Math.max(live, displayedPeak - PEAK_RELEASE_DB);
  $("truePeak").textContent = fmt(displayedPeak, PEAK_FLOOR);
  $("truePeakMax").textContent = fmt(m.truePeakMaxDb, PEAK_FLOOR);

  // Clip indicator latches once the held max crosses the ceiling.
  const ceiling = parseFloat(ceilingInput.value);
  if (Number.isFinite(ceiling) && m.truePeakMaxDb >= ceiling) clipLatched = true;
  tpCard.classList.toggle("clipping", clipLatched);
  clipFlag.classList.toggle("on", clipLatched);

  const target = parseFloat(targetInput.value);
  if (Number.isFinite(target) && m.integrated > LOUDNESS_FLOOR) {
    const gain = target - m.integrated;
    const sign = gain >= 0 ? "+" : "−";
    deltaEl.innerHTML = `<span class="delta-label">apply</span> <strong>${sign}${Math.abs(gain).toFixed(1)} dB</strong>`;
    deltaEl.classList.toggle("hot", Math.abs(gain) > 1);
  } else {
    deltaEl.innerHTML = `<span class="delta-label">apply</span> <strong>—</strong>`;
    deltaEl.classList.remove("hot");
  }
}

function resizeCanvas() {
  const dpr = window.devicePixelRatio || 1;
  const rect = canvas.getBoundingClientRect();
  canvas.width = Math.max(1, Math.round(rect.width * dpr));
  canvas.height = Math.max(1, Math.round(rect.height * dpr));
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
}

// Reference grid lines at musically useful frequencies.
const GRID_HZ = [50, 100, 200, 500, 1000, 2000, 5000, 10000];

function hzToX(hz: number, w: number, nyquist: number): number {
  const fLo = 20;
  const fHi = Math.min(20000, nyquist);
  const t = Math.log(hz / fLo) / Math.log(fHi / fLo);
  return t * w;
}

function drawSpectrum() {
  const w = canvas.clientWidth;
  const h = canvas.clientHeight;
  ctx.clearRect(0, 0, w, h);

  ctx.fillStyle = "#0c0e13";
  ctx.fillRect(0, 0, w, h);

  const nyquist = latest ? latest.sampleRate / 2 : 24000;

  // dB grid lines
  ctx.strokeStyle = "rgba(255,255,255,0.05)";
  ctx.fillStyle = "rgba(255,255,255,0.28)";
  ctx.font = "10px ui-monospace, monospace";
  ctx.lineWidth = 1;
  for (let db = SPECTRUM_TOP; db >= SPECTRUM_FLOOR; db -= 20) {
    const y = ((SPECTRUM_TOP - db) / (SPECTRUM_TOP - SPECTRUM_FLOOR)) * h;
    ctx.beginPath();
    ctx.moveTo(0, y + 0.5);
    ctx.lineTo(w, y + 0.5);
    ctx.stroke();
    ctx.fillText(`${db}`, 4, y + 11);
  }

  // frequency grid lines
  for (const hz of GRID_HZ) {
    if (hz >= nyquist) continue;
    const x = hzToX(hz, w, nyquist);
    ctx.beginPath();
    ctx.moveTo(x + 0.5, 0);
    ctx.lineTo(x + 0.5, h);
    ctx.stroke();
    const label = hz >= 1000 ? `${hz / 1000}k` : `${hz}`;
    ctx.fillText(label, x + 3, h - 4);
  }

  const spec = latest?.spectrum;
  if (!spec || spec.length === 0) return;

  const n = spec.length;
  if (peaks.length !== n) peaks = new Array(n).fill(SPECTRUM_FLOOR);

  const toY = (db: number) =>
    ((SPECTRUM_TOP - db) / (SPECTRUM_TOP - SPECTRUM_FLOOR)) * h;

  const barW = w / n;

  const grad = ctx.createLinearGradient(0, 0, 0, h);
  grad.addColorStop(0, "#ff5d5d");
  grad.addColorStop(0.35, "#ffd24a");
  grad.addColorStop(0.7, "#54e08a");
  grad.addColorStop(1, "#2a9d8f");
  ctx.fillStyle = grad;

  for (let i = 0; i < n; i++) {
    const db = Math.max(SPECTRUM_FLOOR, Math.min(SPECTRUM_TOP, spec[i]));
    const y = toY(db);
    ctx.fillRect(i * barW, y, barW - 1, h - y);

    if (db > peaks[i]) peaks[i] = db;
    else peaks[i] = Math.max(SPECTRUM_FLOOR, peaks[i] - 0.6);
  }

  ctx.fillStyle = "rgba(255,255,255,0.75)";
  for (let i = 0; i < n; i++) {
    const y = toY(peaks[i]);
    ctx.fillRect(i * barW, y - 1, barW - 1, 2);
  }
}

function frame() {
  drawSpectrum();
  requestAnimationFrame(frame);
}

function resetMeasurement() {
  peaks = [];
  displayedPeak = PEAK_FLOOR;
  clipLatched = false;
  tpCard.classList.remove("clipping");
  clipFlag.classList.remove("on");
  invoke("reset_integrated").catch((e) => setStatus(String(e), "err"));
}

window.addEventListener("DOMContentLoaded", async () => {
  resizeCanvas();
  window.addEventListener("resize", resizeCanvas);

  await loadDevices();

  deviceSelect.addEventListener("change", refreshDeviceConfig);
  toggleBtn.addEventListener("click", () => (running ? stop() : start()));
  resetBtn.addEventListener("click", resetMeasurement);
  targetInput.addEventListener("input", () => {
    if (latest) updateReadouts(latest);
  });
  ceilingInput.addEventListener("input", () => {
    if (latest) updateReadouts(latest);
  });

  await listen<Metrics>("meter-update", (event) => {
    latest = event.payload;
    updateReadouts(latest);
  });

  requestAnimationFrame(frame);
});

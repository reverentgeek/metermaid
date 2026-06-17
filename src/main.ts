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
// Ballistics expressed as dB/second so they fall at the same real-world rate
// regardless of display refresh rate or the engine's emit cadence.
const PEAK_RELEASE_DB_PER_SEC = 60; // live true-peak meter fall
const SPECTRUM_PEAK_DECAY_DB_PER_SEC = 36; // spectrum peak-hold fall
// Clamp the per-tick delta so a backgrounded tab (large gap between ticks)
// doesn't make the meters jump on the first frame back.
const MAX_TICK_SEC = 0.1;

let running = false;
let latest: Metrics | null = null;
let peaks: number[] = []; // smoothed spectrum peak-hold per band
let displayedPeak = PEAK_FLOOR; // live true-peak with release ballistics
let lastPeakTs = 0; // timestamp of the last true-peak ballistics update (ms)
let lastFrameTs = 0; // timestamp of the last spectrum frame (ms)
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
    lastPeakTs = 0;
    toggleBtn.textContent = "Stop";
    toggleBtn.classList.add("running");
    configControlsEnabled(false);
    const mode = info.channels === 1 ? "mono" : `${info.channels} ch`;
    setStatus(`${info.sampleRate / 1000} kHz · ${mode}`, "ok");
  } catch (e) {
    setStatus(String(e), "err");
  }
}

// Return the UI to its idle/stopped state. Shared by an explicit Stop and by
// involuntary teardown when the capture device faults.
function teardownRunningUi() {
  running = false;
  latest = null;
  toggleBtn.textContent = "Start";
  toggleBtn.classList.remove("running");
  configControlsEnabled(true);
}

async function stop() {
  try {
    await invoke("stop_capture");
  } catch (e) {
    setStatus(String(e), "err");
  }
  teardownRunningUi();
  setStatus("stopped", "idle");
}

// The audio engine emits this when the OS reports a fault on the active stream
// (e.g. the device is unplugged mid-capture). Tear down and surface why.
function handleStreamError(message: string) {
  if (!running) return;
  void invoke("stop_capture").catch(() => {});
  teardownRunningUi();
  setStatus(`device error: ${message}`, "err");
}

function updateReadouts(m: Metrics) {
  $("integrated").textContent = fmt(m.integrated);
  $("shortTerm").textContent = fmt(m.shortTerm);
  $("momentary").textContent = fmt(m.momentary);
  $("lra").textContent = m.lra > 0 ? m.lra.toFixed(1) : "0.0";

  // Live true peak with release ballistics; held max from the engine.
  const now = performance.now();
  const dt = lastPeakTs ? Math.min((now - lastPeakTs) / 1000, MAX_TICK_SEC) : 0;
  lastPeakTs = now;
  const live = m.truePeakDb;
  displayedPeak =
    live > displayedPeak
      ? live
      : Math.max(live, displayedPeak - PEAK_RELEASE_DB_PER_SEC * dt);
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

// Reference grid lines at musically useful frequencies. `major` ticks get a
// brighter line plus a text label; the rest are faint, unlabeled minor ticks
// that help pinpoint which frequency is spiking without cluttering the axis.
const GRID_HZ: { hz: number; major: boolean }[] = [
  { hz: 20, major: true },
  { hz: 30, major: false },
  { hz: 40, major: false },
  { hz: 50, major: true },
  { hz: 60, major: false },
  { hz: 80, major: false },
  { hz: 100, major: true },
  { hz: 150, major: false },
  { hz: 200, major: true },
  { hz: 300, major: false },
  { hz: 400, major: false },
  { hz: 500, major: true },
  { hz: 700, major: false },
  { hz: 1000, major: true },
  { hz: 1500, major: false },
  { hz: 2000, major: true },
  { hz: 3000, major: false },
  { hz: 4000, major: false },
  { hz: 5000, major: true },
  { hz: 7000, major: false },
  { hz: 10000, major: true },
  { hz: 15000, major: true },
  { hz: 20000, major: true },
];

function fmtHz(hz: number): string {
  if (hz < 1000) return `${hz}`;
  const k = hz / 1000;
  return `${Number.isInteger(k) ? k : k.toFixed(1)}k`;
}

function hzToX(hz: number, w: number, nyquist: number): number {
  const fLo = 20;
  const fHi = Math.min(20000, nyquist);
  const t = Math.log(hz / fLo) / Math.log(fHi / fLo);
  return t * w;
}

// Gutters reserved outside the plot area so axis labels stay legible — the
// bars never draw over them. Left holds the dB scale, bottom the frequencies.
const PLOT_PAD_LEFT = 28;
const PLOT_PAD_BOTTOM = 14;
const PLOT_PAD_TOP = 4;
const PLOT_PAD_RIGHT = 14;

function drawSpectrum(dt: number) {
  const w = canvas.clientWidth;
  const h = canvas.clientHeight;
  ctx.clearRect(0, 0, w, h);

  ctx.fillStyle = "#0c0e13";
  ctx.fillRect(0, 0, w, h);

  // Inner plot rectangle; everything data-driven is drawn inside this.
  const pl = PLOT_PAD_LEFT;
  const pt = PLOT_PAD_TOP;
  const pw = Math.max(1, w - PLOT_PAD_LEFT - PLOT_PAD_RIGHT);
  const ph = Math.max(1, h - PLOT_PAD_TOP - PLOT_PAD_BOTTOM);
  const pb = pt + ph; // plot bottom

  const nyquist = latest ? latest.sampleRate / 2 : 24000;
  const toY = (db: number) =>
    pt + ((SPECTRUM_TOP - db) / (SPECTRUM_TOP - SPECTRUM_FLOOR)) * ph;

  ctx.font = "10px ui-monospace, monospace";
  ctx.lineWidth = 1;

  // dB grid lines + labels in the left gutter
  ctx.strokeStyle = "rgba(255,255,255,0.05)";
  ctx.textBaseline = "middle";
  ctx.textAlign = "right";
  for (let db = SPECTRUM_TOP; db >= SPECTRUM_FLOOR; db -= 20) {
    const y = toY(db);
    ctx.beginPath();
    ctx.moveTo(pl, y + 0.5);
    ctx.lineTo(pl + pw, y + 0.5);
    ctx.stroke();
    ctx.fillStyle = "rgba(255,255,255,0.32)";
    ctx.fillText(`${db}`, pl - 4, y);
  }

  // frequency grid lines + labels in the bottom gutter
  ctx.textBaseline = "alphabetic";
  ctx.textAlign = "center";
  let lastLabelX = -Infinity;
  for (const { hz, major } of GRID_HZ) {
    if (hz >= nyquist) continue;
    const x = pl + hzToX(hz, pw, nyquist);
    ctx.strokeStyle = major ? "rgba(255,255,255,0.10)" : "rgba(255,255,255,0.04)";
    ctx.beginPath();
    ctx.moveTo(x + 0.5, pt);
    ctx.lineTo(x + 0.5, pb);
    ctx.stroke();
    // Only label major ticks, and skip any that would crowd the previous label
    // (the log scale compresses the high end where labels would otherwise overlap).
    if (major && x - lastLabelX >= 24) {
      ctx.fillStyle = "rgba(255,255,255,0.5)";
      ctx.fillText(fmtHz(hz), x, h - 3);
      lastLabelX = x;
    }
  }
  ctx.textAlign = "left";

  const spec = latest?.spectrum;
  if (!spec || spec.length === 0) return;

  const n = spec.length;
  if (peaks.length !== n) peaks = new Array(n).fill(SPECTRUM_FLOOR);

  const barW = pw / n;

  const grad = ctx.createLinearGradient(0, pt, 0, pb);
  grad.addColorStop(0, "#ff5d5d");
  grad.addColorStop(0.35, "#ffd24a");
  grad.addColorStop(0.7, "#54e08a");
  grad.addColorStop(1, "#2a9d8f");
  ctx.fillStyle = grad;

  for (let i = 0; i < n; i++) {
    const db = Math.max(SPECTRUM_FLOOR, Math.min(SPECTRUM_TOP, spec[i]));
    const y = toY(db);
    ctx.fillRect(pl + i * barW, y, barW - 1, pb - y);

    if (db > peaks[i]) peaks[i] = db;
    else peaks[i] = Math.max(SPECTRUM_FLOOR, peaks[i] - SPECTRUM_PEAK_DECAY_DB_PER_SEC * dt);
  }

  ctx.fillStyle = "rgba(255,255,255,0.75)";
  for (let i = 0; i < n; i++) {
    const y = toY(peaks[i]);
    ctx.fillRect(pl + i * barW, y - 1, barW - 1, 2);
  }
}

function frame(now: number) {
  const dt = lastFrameTs ? Math.min((now - lastFrameTs) / 1000, MAX_TICK_SEC) : 0;
  lastFrameTs = now;
  drawSpectrum(dt);
  requestAnimationFrame(frame);
}

function resetMeasurement() {
  peaks = [];
  displayedPeak = PEAK_FLOOR;
  lastPeakTs = 0;
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

  await listen<string>("stream-error", (event) => {
    handleStreamError(event.payload);
  });

  requestAnimationFrame(frame);
});

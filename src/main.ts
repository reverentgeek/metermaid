// Bundled fonts (Latin subset) so the UI looks identical on macOS, Windows, and
// Linux instead of falling back to each OS's default sans/mono. Inter for the
// chrome, JetBrains Mono for the numeric readouts and spectrum labels.
import "@fontsource/inter/latin-400.css";
import "@fontsource/inter/latin-600.css";
import "@fontsource/inter/latin-700.css";
import "@fontsource/jetbrains-mono/latin-400.css";
import "@fontsource/jetbrains-mono/latin-600.css";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { load, type Store } from "@tauri-apps/plugin-store";

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

const $ = <T extends HTMLElement>(id: string) =>
	document.getElementById(id) as T;

const deviceSelect = $<HTMLSelectElement>("device");
const channelSelect = $<HTMLSelectElement>("channels");
const rateSelect = $<HTMLSelectElement>("rate");
const startBtn = $<HTMLButtonElement>("start");
const resetBtn = $<HTMLButtonElement>("reset");
const stopBtn = $<HTMLButtonElement>("stop");
const statusEl = $<HTMLSpanElement>("status");
const targetInput = $<HTMLInputElement>("target");
const ceilingInput = $<HTMLInputElement>("ceiling");
const deltaEl = $<HTMLDivElement>("delta");
const tpCard = $<HTMLDivElement>("tpCard");
const clipFlag = $<HTMLSpanElement>("clipFlag");
const autostartInput = $<HTMLInputElement>("autostart");
const errorBanner = $<HTMLDivElement>("errorBanner");
const errorMessage = $<HTMLSpanElement>("errorMessage");
const errorCopy = $<HTMLButtonElement>("errorCopy");
const errorDismiss = $<HTMLButtonElement>("errorDismiss");
const resetHint = $<HTMLDivElement>("resetHint");
const hintDismiss = $<HTMLButtonElement>("hintDismiss");
const canvas = $<HTMLCanvasElement>("spectrum");
const ctx = canvas.getContext("2d")!;

// ---- Persisted settings (tauri-plugin-store) -----------------------------
// Device/channel/rate/target/ceiling/auto-start survive across launches. We
// keep a single JSON store and write the full control state on any change.
let store: Store | null = null;
// Suppresses persistence while we apply restored values to the controls, so
// the act of restoring doesn't immediately rewrite the store.
let restoring = true;
// Saved selections waiting to be reapplied as the device list / config load.
// Each is consumed once and cleared so later user edits use plain defaults.
let pendingDevice: string | undefined;
let pendingChannels: string | undefined;
let pendingRate: number | undefined;
// Whether the saved device is actually present this launch (gates auto-start).
let savedDeviceAvailable = true;
// One-time onboarding: show the "Reset between patches" hint until the user has
// seen it once (dismissed it or used Reset). Persisted so it never nags again.
let resetHintSeen = false;

const numOrNull = (s: string) => {
	const n = parseFloat(s);
	return Number.isFinite(n) ? n : null;
};

async function persist() {
	if (!store || restoring) return;
	try {
		await store.set("device", deviceSelect.value || "");
		await store.set("channels", channelSelect.value);
		await store.set(
			"sampleRate",
			rateSelect.value ? Number(rateSelect.value) : null,
		);
		await store.set("target", numOrNull(targetInput.value));
		await store.set("ceiling", numOrNull(ceilingInput.value));
		await store.set("autoStart", autostartInput.checked);
		await store.save();
	} catch {
		// Persistence is best-effort; never block metering on a failed write.
	}
}

function fmt(v: number, floor = LOUDNESS_FLOOR): string {
	if (!Number.isFinite(v) || v <= floor) return "−∞";
	return v.toFixed(1);
}

function setStatus(text: string, kind: "ok" | "err" | "idle") {
	statusEl.textContent = text;
	statusEl.className = `status status-${kind}`;
}

// Tauri command rejections are usually plain strings (our Rust commands return
// `Result<_, String>`), but normalize anything else so the user never sees an
// unhelpful "[object Object]".
function errText(e: unknown): string {
	if (typeof e === "string") return e;
	if (e instanceof Error) return e.message;
	if (e && typeof e === "object") {
		const msg = (e as { message?: unknown }).message;
		if (typeof msg === "string") return msg;
		try {
			return JSON.stringify(e);
		} catch {
			/* fall through to String() */
		}
	}
	return String(e);
}

function hideError() {
	errorBanner.hidden = true;
	errorMessage.textContent = "";
}

// Surface a failure the user can actually read, copy, and report. `context` is
// a short label for what was attempted ("Start capture"); the toolbar status
// stays terse while the banner shows the full, selectable detail. Also logs to
// the console so it's recoverable from a dev build / webview inspector.
function reportError(context: string, e: unknown) {
	const detail = errText(e);
	console.error(`${context}:`, e);
	setStatus("error", "err");
	errorMessage.textContent = `${context}: ${detail}`;
	errorBanner.hidden = false;
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
		// Restore the saved device if it's still present; otherwise leave the
		// system default selected and surface a notice. When the saved device is
		// gone we also drop the saved channels/rate so the default device gets its
		// own sensible defaults rather than a mismatched selection.
		if (pendingDevice) {
			if (devices.some((d) => d.name === pendingDevice)) {
				deviceSelect.value = pendingDevice;
			} else {
				savedDeviceAvailable = false;
				pendingChannels = undefined;
				pendingRate = undefined;
				setStatus("saved device unavailable — using default", "err");
			}
			pendingDevice = undefined;
		}
		await refreshDeviceConfig();
	} catch (e) {
		reportError("List input devices", e);
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

		// Reapply saved channel/rate selections, but only if they're still valid
		// for this device — otherwise the defaults chosen above stand. Consumed
		// once: subsequent device switches fall through to plain defaults.
		if (pendingChannels !== undefined) {
			if (
				Array.from(channelSelect.options).some(
					(o) => o.value === pendingChannels,
				)
			) {
				channelSelect.value = pendingChannels;
			}
			pendingChannels = undefined;
		}
		if (pendingRate !== undefined) {
			if (cfg.sampleRates.includes(pendingRate)) {
				rateSelect.value = String(pendingRate);
			}
			pendingRate = undefined;
		}
	} catch (e) {
		reportError("Read device settings", e);
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
		// Reset becomes the in-session primary; Stop is a quiet secondary.
		startBtn.hidden = true;
		resetBtn.hidden = false;
		stopBtn.hidden = false;
		configControlsEnabled(false);
		const mode = info.channels === 1 ? "mono" : `${info.channels} ch`;
		setStatus(`${info.sampleRate / 1000} kHz · ${mode}`, "ok");
		hideError();
		maybeShowResetHint();
	} catch (e) {
		reportError("Start capture", e);
	}
}

// Show the "Reset between patches" hint once, the first time capture starts.
function maybeShowResetHint() {
	if (!resetHintSeen) resetHint.hidden = false;
}

// Hide the hint and remember it's been seen, so it never shows again. Called
// when the user dismisses it or uses Reset for the first time.
async function markResetHintSeen() {
	resetHint.hidden = true;
	if (resetHintSeen) return;
	resetHintSeen = true;
	if (!store) return;
	try {
		await store.set("resetHintSeen", true);
		await store.save();
	} catch {
		// Best-effort; the hint reappearing next session is harmless.
	}
}

// Return the UI to its idle/stopped state. Shared by an explicit Stop and by
// involuntary teardown when the capture device faults.
function teardownRunningUi() {
	running = false;
	latest = null;
	startBtn.hidden = false;
	resetBtn.hidden = true;
	stopBtn.hidden = true;
	// Tuck the hint away with the session; seen-state is untouched so a user who
	// never engaged still gets it next time.
	resetHint.hidden = true;
	configControlsEnabled(true);
}

async function stop() {
	try {
		await invoke("stop_capture");
	} catch (e) {
		reportError("Stop capture", e);
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
	reportError("Audio device", message);
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
	if (Number.isFinite(ceiling) && m.truePeakMaxDb >= ceiling)
		clipLatched = true;
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

	ctx.font = '10px "JetBrains Mono", ui-monospace, monospace';
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
		ctx.strokeStyle = major
			? "rgba(255,255,255,0.10)"
			: "rgba(255,255,255,0.04)";
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
		else
			peaks[i] = Math.max(
				SPECTRUM_FLOOR,
				peaks[i] - SPECTRUM_PEAK_DECAY_DB_PER_SEC * dt,
			);
	}

	ctx.fillStyle = "rgba(255,255,255,0.75)";
	for (let i = 0; i < n; i++) {
		const y = toY(peaks[i]);
		ctx.fillRect(pl + i * barW, y - 1, barW - 1, 2);
	}
}

function frame(now: number) {
	const dt = lastFrameTs
		? Math.min((now - lastFrameTs) / 1000, MAX_TICK_SEC)
		: 0;
	lastFrameTs = now;
	drawSpectrum(dt);
	requestAnimationFrame(frame);
}

function resetMeasurement() {
	// Using Reset means the workflow has been learned — retire the hint for good.
	void markResetHintSeen();
	peaks = [];
	displayedPeak = PEAK_FLOOR;
	lastPeakTs = 0;
	clipLatched = false;
	tpCard.classList.remove("clipping");
	clipFlag.classList.remove("on");
	invoke("reset_integrated").catch((e) => reportError("Reset measurement", e));
}

window.addEventListener("DOMContentLoaded", async () => {
	resizeCanvas();
	window.addEventListener("resize", resizeCanvas);

	// Load persisted settings before touching the controls. Target/ceiling apply
	// immediately; device/channels/rate are staged as "pending" and reapplied as
	// the device list and per-device config load (with validation) below.
	try {
		store = await load("settings.json");
		const dev = (await store.get<string>("device")) ?? "";
		const ch = await store.get<string>("channels");
		const sr = await store.get<number>("sampleRate");
		const tgt = await store.get<number>("target");
		const ceil = await store.get<number>("ceiling");
		const auto = await store.get<boolean>("autoStart");
		const hintSeen = await store.get<boolean>("resetHintSeen");
		if (dev) pendingDevice = dev;
		if (typeof ch === "string") pendingChannels = ch;
		if (typeof sr === "number") pendingRate = sr;
		if (typeof tgt === "number") targetInput.value = String(tgt);
		if (typeof ceil === "number") ceilingInput.value = String(ceil);
		autostartInput.checked = auto === true;
		resetHintSeen = hintSeen === true;
	} catch {
		// No store yet (first launch) or a read error — fall back to UI defaults.
	}

	await loadDevices();

	// Restore is complete; allow control changes to persist from here on.
	restoring = false;

	deviceSelect.addEventListener("change", async () => {
		await refreshDeviceConfig();
		void persist();
	});
	startBtn.addEventListener("click", () => void start());
	stopBtn.addEventListener("click", () => void stop());
	resetBtn.addEventListener("click", resetMeasurement);
	hintDismiss.addEventListener("click", () => void markResetHintSeen());
	errorDismiss.addEventListener("click", hideError);
	errorCopy.addEventListener("click", async () => {
		try {
			await navigator.clipboard.writeText(errorMessage.textContent ?? "");
			errorCopy.textContent = "Copied";
			setTimeout(() => (errorCopy.textContent = "Copy"), 1500);
		} catch {
			// Clipboard unavailable — the message is selectable in the banner.
		}
	});
	// Custom number steppers: nudge the target input by its step, then fire the
	// same input/change events typing would, so readouts update and the value
	// persists through the existing listeners.
	for (const btn of document.querySelectorAll<HTMLButtonElement>(".num-step")) {
		btn.addEventListener("click", () => {
			const input = $<HTMLInputElement>(btn.dataset.for ?? "");
			if (!input) return;
			if (btn.dataset.dir === "up") input.stepUp();
			else input.stepDown();
			input.dispatchEvent(new Event("input", { bubbles: true }));
			input.dispatchEvent(new Event("change", { bubbles: true }));
		});
	}

	channelSelect.addEventListener("change", () => void persist());
	rateSelect.addEventListener("change", () => void persist());
	autostartInput.addEventListener("change", () => void persist());
	targetInput.addEventListener("input", () => {
		if (latest) updateReadouts(latest);
	});
	targetInput.addEventListener("change", () => void persist());
	ceilingInput.addEventListener("input", () => {
		if (latest) updateReadouts(latest);
	});
	ceilingInput.addEventListener("change", () => void persist());

	// Optional: auto-start capture when a valid saved device + channels restored.
	if (
		autostartInput.checked &&
		savedDeviceAvailable &&
		!deviceSelect.disabled &&
		deviceSelect.value &&
		channelSelect.value
	) {
		await start();
	}

	await listen<Metrics>("meter-update", (event) => {
		latest = event.payload;
		updateReadouts(latest);
	});

	await listen<string>("stream-error", (event) => {
		handleStreamError(event.payload);
	});

	// Wait for the bundled fonts before the first draw so the canvas spectrum
	// labels render in JetBrains Mono rather than briefly flashing a fallback.
	await document.fonts.ready;
	requestAnimationFrame(frame);
});

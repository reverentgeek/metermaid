// Bundled fonts (Latin subset) so the UI looks identical on macOS, Windows, and
// Linux instead of falling back to each OS's default sans/mono. Inter for the
// chrome, JetBrains Mono for the numeric readouts and spectrum labels.
import "@fontsource/inter/latin-400.css";
import "@fontsource/inter/latin-600.css";
import "@fontsource/inter/latin-700.css";
import "@fontsource/jetbrains-mono/latin-400.css";
import "@fontsource/jetbrains-mono/latin-600.css";
import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { relaunch } from "@tauri-apps/plugin-process";
import { load, type Store } from "@tauri-apps/plugin-store";
import { check, type Update } from "@tauri-apps/plugin-updater";

interface DeviceInfo {
	// Host-qualified identity: the <option> value and the persisted settings key.
	// ASIO devices are prefixed (`asio:`); default-host devices use the bare name.
	id: string;
	name: string;
	// "default" or "asio" — drives the disambiguating label.
	host: string;
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
	generation: number;
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
// Measurement generation of the most recent meter-update. The engine bumps it
// on every stream (re)configure and Reset, so metrics computed *before* a
// Reset we requested are identifiable while still in flight.
let lastGeneration = 0;
// Clip latching is suppressed for metrics at or below this generation: after a
// Reset (or a fresh Start), a stale frame still in flight carries the old held
// true-peak max, which would instantly re-latch the clip light just cleared.
let latchHoldGeneration = -1;

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
const updateCheckBtn = $<HTMLButtonElement>("updateCheck");
const updateBanner = $<HTMLDivElement>("updateBanner");
const updateMessage = $<HTMLSpanElement>("updateMessage");
const updateNotes = $<HTMLSpanElement>("updateNotes");
const updateInstall = $<HTMLButtonElement>("updateInstall");
const updateDismiss = $<HTMLButtonElement>("updateDismiss");
const aboutModal = $<HTMLDivElement>("aboutModal");
const aboutClose = $<HTMLButtonElement>("aboutClose");
const aboutVersion = $<HTMLSpanElement>("aboutVersion");
const aboutAsio = $<HTMLParagraphElement>("aboutAsio");
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

// ---- Self-update (tauri-plugin-updater) ----------------------------------
// On launch and on demand we ask GitHub Releases (via the `latest.json` the
// release workflow publishes) whether a newer signed build exists. If so, a
// banner offers a one-click download + relaunch. The download and minisign
// verification happen in Rust; failures here are non-fatal — being offline, or
// running a dev build with no newer published release, simply leaves the app
// unchanged. `currentUpdate` holds the pending handle between check and install.
let currentUpdate: Update | null = null;
let updateInFlight = false;

function showUpdateBanner(update: Update) {
	updateMessage.textContent = `MeterMaid ${update.version} is available${
		update.currentVersion ? ` (you have ${update.currentVersion})` : ""
	}.`;
	updateNotes.textContent = (update.body ?? "").trim();
	updateNotes.hidden = updateNotes.textContent === "";
	updateBanner.hidden = false;
}

// `manual` distinguishes the user clicking "Check for updates" (which deserves
// visible feedback, including "up to date" and surfaced errors) from the silent
// check on launch (which stays quiet unless an update is actually found).
async function checkForUpdates(manual: boolean) {
	if (updateInFlight) return;
	if (manual) {
		updateCheckBtn.disabled = true;
		updateCheckBtn.textContent = "Checking…";
	}
	try {
		const update = await check();
		if (update) {
			currentUpdate = update;
			showUpdateBanner(update);
		} else if (manual) {
			// Brief inline confirmation; the toolbar status is reserved for capture.
			updateCheckBtn.textContent = "Up to date";
			setTimeout(() => {
				updateCheckBtn.textContent = "Check for updates";
			}, 2000);
		}
	} catch (e) {
		if (manual) reportError("Check for updates", e);
	} finally {
		if (manual) {
			updateCheckBtn.disabled = false;
			if (updateCheckBtn.textContent === "Checking…") {
				updateCheckBtn.textContent = "Check for updates";
			}
		}
	}
}

async function installUpdate() {
	if (!currentUpdate || updateInFlight) return;
	updateInFlight = true;
	updateInstall.disabled = true;
	updateDismiss.disabled = true;
	updateInstall.textContent = "Downloading…";
	try {
		let downloaded = 0;
		let total = 0;
		await currentUpdate.downloadAndInstall((event) => {
			switch (event.event) {
				case "Started":
					total = event.data.contentLength ?? 0;
					break;
				case "Progress":
					downloaded += event.data.chunkLength;
					updateInstall.textContent = total
						? `Downloading… ${Math.round((downloaded / total) * 100)}%`
						: "Downloading…";
					break;
				case "Finished":
					updateInstall.textContent = "Installing…";
					break;
			}
		});
		// On macOS/Linux the installed bundle is staged and we relaunch into it.
		// (On Windows the NSIS installer takes over and exits the app itself.)
		await relaunch();
	} catch (e) {
		reportError("Install update", e);
		updateInFlight = false;
		updateInstall.disabled = false;
		updateDismiss.disabled = false;
		updateInstall.textContent = "Install & Restart";
	}
}

// ---- About dialog --------------------------------------------------------
// A custom, centered in-app dialog opened from the macOS "About MeterMaid" menu
// (via the `menu-about` event). Replaces the native panel so the text can be
// centered and the links are real, clickable links opened in the user's browser
// through the opener plugin.
function showAbout() {
	aboutModal.hidden = false;
}

function hideAbout() {
	aboutModal.hidden = true;
}

function configControlsEnabled(enabled: boolean) {
	deviceSelect.disabled = !enabled;
	channelSelect.disabled = !enabled;
	rateSelect.disabled = !enabled;
}

// Signature of just the default-host (WASAPI/CoreAudio/ALSA) devices, so the
// hotplug poller can skip rebuilding when nothing has changed. Enumerating ASIO
// loads its driver, so we watch only the default host here and do a full
// (ASIO-inclusive) refresh when that topology actually changes.
let lastWasapiSig = "";

// Whether the toolbar is currently showing a device-availability notice (e.g.
// "input device disconnected"). Tracked so we can clear it back to idle once the
// device returns or the user picks another one.
let deviceNotice = false;

function noteDeviceIssue(text: string) {
	deviceNotice = true;
	setStatus(text, "err");
}

// Clear a lingering device notice once the situation resolves. Leaves a real
// error banner (and any active capture status) untouched.
function clearDeviceNotice() {
	if (!deviceNotice) return;
	deviceNotice = false;
	if (!running && errorBanner.hidden) setStatus("stopped", "idle");
}

function deviceSig(devices: DeviceInfo[]): string {
	return devices.map((d) => `${d.isDefault ? "*" : ""}${d.id}`).join("\n");
}

// Display label for a device option. ASIO devices are tagged so they're
// distinguishable from the same hardware's WASAPI endpoint; the default device
// is annotated too.
function deviceLabel(d: DeviceInfo): string {
	if (d.host === "asio") return `${d.name} (ASIO)`;
	return d.isDefault ? `${d.name} (default)` : d.name;
}

// (Re)render the device <option>s. If `keep` is the id of a device that's still
// present it stays selected; otherwise the system default (or first device)
// is selected. Shared by the initial load and the hotplug poller.
function renderDeviceOptions(devices: DeviceInfo[], keep: string) {
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
		opt.value = d.id;
		opt.textContent = deviceLabel(d);
		deviceSelect.append(opt);
	}
	if (keep && devices.some((d) => d.id === keep)) {
		deviceSelect.value = keep;
	} else {
		const def = devices.find((d) => d.isDefault) ?? devices[0];
		deviceSelect.value = def.id;
	}
}

async function loadDevices() {
	try {
		const devices = await invoke<DeviceInfo[]>("list_devices", {
			includeAsio: true,
		});
		renderDeviceOptions(devices, "");
		lastWasapiSig = deviceSig(devices.filter((d) => d.host !== "asio"));
		if (devices.length === 0) return;
		// Restore the saved device if it's still present; otherwise leave the
		// system default selected and surface a notice. When the saved device is
		// gone we also drop the saved channels/rate so the default device gets its
		// own sensible defaults rather than a mismatched selection.
		if (pendingDevice) {
			if (devices.some((d) => d.id === pendingDevice)) {
				deviceSelect.value = pendingDevice;
			} else {
				savedDeviceAvailable = false;
				pendingChannels = undefined;
				pendingRate = undefined;
				noteDeviceIssue("saved device unavailable — using default");
			}
			pendingDevice = undefined;
		}
		await refreshDeviceConfig();
	} catch (e) {
		reportError("List input devices", e);
	}
}

// How often to re-check the input-device list for hotplug changes.
const DEVICE_POLL_MS = 2000;

// cpal exposes no device-change callback, so we poll for USB/audio devices
// being plugged in or removed while the app is idle. We re-list on an interval
// and rebuild the dropdown only when the set actually changes, preserving the
// current selection. Skipped during capture (the config controls are locked
// then) and during restore; a device vanishing mid-capture is already handled
// by the stream-error path.
async function pollDevices() {
	if (running || restoring || deviceSelect.disabled) return;
	// Cheap pass: only the default host (no ASIO driver loading). If the
	// default-host set is unchanged, nothing to do — ASIO devices are stable
	// between these ticks and don't need re-enumerating.
	let wasapi: DeviceInfo[];
	try {
		wasapi = await invoke<DeviceInfo[]>("list_devices", { includeAsio: false });
	} catch {
		return; // Transient failure — try again on the next tick.
	}
	const wasapiSig = deviceSig(wasapi);
	if (wasapiSig === lastWasapiSig) return;
	lastWasapiSig = wasapiSig;
	// Topology changed (e.g. an interface was plugged in) — now do the full,
	// ASIO-inclusive enumeration to rebuild the dropdown.
	let devices: DeviceInfo[];
	try {
		devices = await invoke<DeviceInfo[]>("list_devices", { includeAsio: true });
	} catch {
		return;
	}
	const prev = deviceSelect.value;
	renderDeviceOptions(devices, prev);
	if (deviceSelect.value !== prev) {
		// The selection changed: the previously selected device went away (or the
		// first device just appeared). Resync the channel/rate pickers, and tell
		// the user when an active choice was dropped.
		if (prev) noteDeviceIssue("input device disconnected — using default");
		else clearDeviceNotice();
		await refreshDeviceConfig();
	} else {
		// Selection survived — the device topology changed but our pick is still
		// here (e.g. it was just reconnected). Retire any stale notice.
		clearDeviceNotice();
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
		latchHoldGeneration = lastGeneration;
		displayedPeak = PEAK_FLOOR;
		lastPeakTs = 0;
		// Drop the previous session's spectrum peak-hold so the new capture
		// starts clean rather than under a stale, decaying peak line.
		peaks = [];
		// Reset becomes the in-session primary; Stop is a quiet secondary.
		startBtn.hidden = true;
		resetBtn.hidden = false;
		stopBtn.hidden = false;
		configControlsEnabled(false);
		const mode = info.channels === 1 ? "mono" : `${info.channels} ch`;
		setStatus(`${info.sampleRate / 1000} kHz · ${mode}`, "ok");
		hideError();
		maybeShowResetHint();
		requestFrame(); // start the render loop (it self-sustains while running)
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
	requestFrame(); // one final repaint to clear the bars, then the loop idles
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

// The audio engine emits this when the OS reports a fault that ends the
// active stream (e.g. the device is unplugged mid-capture) — recoverable
// advisories like buffer over/underruns are filtered out engine-side
// (is_fatal_stream_error). Tear down and surface why.
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

	// Clip indicator latches once the held max crosses the ceiling — but only
	// for metrics computed after the latest Reset/Start (see latchHoldGeneration).
	lastGeneration = m.generation;
	const ceiling = parseFloat(ceilingInput.value);
	if (
		m.generation > latchHoldGeneration &&
		Number.isFinite(ceiling) &&
		m.truePeakMaxDb >= ceiling
	)
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
	const w = Math.max(1, Math.round(rect.width * dpr));
	const h = Math.max(1, Math.round(rect.height * dpr));
	// Skip when nothing changed — assigning canvas.width/height clears the buffer
	// even if the value is identical, and the ResizeObserver below fires an
	// initial observation plus every layout tick, so guard against pointless work.
	if (w === canvas.width && h === canvas.height) return;
	canvas.width = w;
	canvas.height = h;
	ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
	// Repaint at the new size; while idle the loop is stopped, so without this
	// the canvas would stay blank/stretched until the next capture.
	requestFrame();
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

let rafPending = false;

// Schedule a single spectrum repaint, coalescing repeated requests within the
// same frame. While capturing, `frame` re-schedules itself for smooth
// animation; when idle it draws once and stops, so the canvas isn't redrawn at
// the display refresh rate for a static, data-less plot — that idle redraw was
// burning several percent CPU (and GPU) for nothing while metering was stopped.
function requestFrame() {
	if (rafPending) return;
	rafPending = true;
	requestAnimationFrame(frame);
}

function frame(now: number) {
	rafPending = false;
	const dt = lastFrameTs
		? Math.min((now - lastFrameTs) / 1000, MAX_TICK_SEC)
		: 0;
	lastFrameTs = now;
	drawSpectrum(dt);
	// Keep animating only while capturing (live bars + peak-hold decay). Idle,
	// the plot is static, so stop until a state change requests a repaint.
	if (running) {
		requestFrame();
	} else {
		lastFrameTs = 0; // next repaint starts fresh (dt = 0), no decay jump
	}
}

function resetMeasurement() {
	// Using Reset means the workflow has been learned — retire the hint for good.
	void markResetHintSeen();
	peaks = [];
	displayedPeak = PEAK_FLOOR;
	lastPeakTs = 0;
	clipLatched = false;
	// Suppress clip latching for frames from before this reset; the engine
	// bumps the generation when it processes it (see latchHoldGeneration).
	latchHoldGeneration = lastGeneration;
	tpCard.classList.remove("clipping");
	clipFlag.classList.remove("on");
	invoke("reset_integrated").catch((e) => reportError("Reset measurement", e));
}

window.addEventListener("DOMContentLoaded", async () => {
	resizeCanvas();
	// `resize` only fires for window-level changes (and catches DPR changes when
	// dragging between monitors). It misses layout-driven size changes of the
	// flex-sized canvas — e.g. when bundled fonts finish loading and reflow the
	// header — which left the backing store stale and the spectrum blank/cropped
	// until a manual window resize. Observe the element itself so any size change
	// re-syncs the backing store.
	window.addEventListener("resize", resizeCanvas);
	new ResizeObserver(resizeCanvas).observe(canvas);

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
		clearDeviceNotice();
		void persist();
	});
	startBtn.addEventListener("click", () => void start());
	stopBtn.addEventListener("click", () => void stop());
	resetBtn.addEventListener("click", resetMeasurement);
	hintDismiss.addEventListener("click", () => void markResetHintSeen());
	updateCheckBtn.addEventListener("click", () => void checkForUpdates(true));
	updateInstall.addEventListener("click", () => void installUpdate());
	updateDismiss.addEventListener("click", () => {
		updateBanner.hidden = true;
	});

	// About dialog: links open in the system browser (not the webview), and a
	// backdrop click, the close button, or Escape dismisses it.
	void getVersion().then((v) => {
		aboutVersion.textContent = v;
	});
	// Surface the ASIO trademark + GPLv3 notice only on the build that links ASIO
	// (x64 Windows); that binary is GPLv3 while the rest stay MIT.
	void invoke<boolean>("asio_build")
		.then((on) => {
			if (on) aboutAsio.hidden = false;
		})
		.catch(() => {});
	aboutModal.addEventListener("click", (e) => {
		const target = e.target as HTMLElement;
		const link = target.closest<HTMLAnchorElement>(".about-link");
		if (link) {
			e.preventDefault();
			void openUrl(link.href);
			return;
		}
		if (target === aboutModal) hideAbout();
	});
	aboutClose.addEventListener("click", hideAbout);
	document.addEventListener("keydown", (e) => {
		if (e.key === "Escape" && !aboutModal.hidden) hideAbout();
		// Space resets the measurement — the leveling loop hits Reset constantly
		// between patches, so a shortcut tightens the core workflow. Gated on the
		// Reset button being visible (i.e. capturing) and on focus not being in a
		// field/button, so Space still types/activates normally there.
		if (e.code === "Space" && !resetBtn.hidden) {
			const t = e.target as HTMLElement | null;
			const tag = t?.tagName;
			if (tag !== "INPUT" && tag !== "BUTTON" && tag !== "SELECT") {
				e.preventDefault();
				resetMeasurement();
			}
		}
	});

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

	await listen<Metrics>("meter-update", (event) => {
		latest = event.payload;
		updateReadouts(latest);
	});

	await listen<string>("stream-error", (event) => {
		handleStreamError(event.payload);
	});

	// The macOS "Check for Updates…" menu item routes here so it shares the
	// in-app button's flow (banner, "up to date"/error feedback).
	await listen("menu-check-updates", () => void checkForUpdates(true));

	// The macOS "About MeterMaid" menu item opens the in-app About dialog.
	await listen("menu-about", () => showAbout());

	// Watch for input devices being plugged in or removed while idle.
	window.setInterval(() => void pollDevices(), DEVICE_POLL_MS);

	// Optional: auto-start capture when a valid saved device + channels restored.
	// Done after the meter-update/stream-error listeners are registered so an
	// immediate stream fault on start is surfaced rather than missed — otherwise
	// the UI could sit with controls disabled and no visible failure.
	if (
		autostartInput.checked &&
		savedDeviceAvailable &&
		!deviceSelect.disabled &&
		deviceSelect.value &&
		channelSelect.value
	) {
		await start();
	}

	// Quietly check for a newer release in the background. Non-blocking and
	// failure-silent (see checkForUpdates); a found update raises the banner.
	void checkForUpdates(false);

	// Wait for the bundled fonts before the first draw so the canvas spectrum
	// labels render in JetBrains Mono rather than briefly flashing a fallback.
	await document.fonts.ready;
	requestFrame();
});

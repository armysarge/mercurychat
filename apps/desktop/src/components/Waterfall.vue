<script setup lang="ts">
// Spectrum and waterfall (OBS-02, M3a Task 12), side by side in the station
// strip: the spectrum is dB against audio frequency (0 to half the sample
// rate, about 4 kHz from Mercury), the waterfall the same feed over time.
// Collapsed by default. While it is open and Mercury's telemetry is Up, the
// shell forwards spectra at most 5 a second (set_waterfall_visible); closing
// it, hiding the window or losing telemetry stops forwarding and drawing. With
// no telemetry it shows the link's state and draws nothing: there is never
// synthetic data. Under reduced motion it redraws at most once a second.
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { computed, nextTick, onMounted, onUnmounted, ref, useId, watch } from "vue";
import { useI18n } from "vue-i18n";
import { claimSpectrum } from "../spectrumDemand";
import { snapshot } from "../store";

/** `large`: the station home's full-width form (owner, 24 Nov: bigger), open unless closed there. */
const props = defineProps<{ shown: boolean; large?: boolean }>();
const { t } = useI18n();

type Frame = { fftSize: number; sampleRate: number; bins: number[] };
const DB_MIN = -120;
const DB_MAX = -20;
/** Waterfall rows kept (and drawn): one per forwarded frame. */
const ROWS = 120;

const KEY = props.large ? "mc.waterfallOpenHome" : "mc.waterfallOpen";
const open = ref((() => {
  try {
    const v = localStorage.getItem(KEY);
    return props.large ? v !== "0" : v === "1";
  } catch {
    return !!props.large;
  }
})());
watch(open, (v) => {
  try {
    localStorage.setItem(KEY, v ? "1" : "0");
  } catch {
    // Storage may be unavailable: the choice then lasts for this run only.
  }
});

const link = computed(() => snapshot.value?.telemetry.link.type ?? "unconfigured");
const up = computed(() => link.value === "up");
const pageVisible = ref(document.visibilityState === "visible");
const reduced = window.matchMedia("(prefers-reduced-motion: reduce)");
const reduceMotion = ref(reduced.matches);
/** Forward and draw only while all of these hold. */
const active = computed(() => props.shown && open.value && up.value && pageVisible.value);

const spectrumCanvas = ref<HTMLCanvasElement | null>(null);
const waterfallCanvas = ref<HTMLCanvasElement | null>(null);
const rows: Float32Array[] = [];
/** A frame has been drawn since the panel opened. */
const drawing = ref(false);
let lastDraw = 0;
let unlisten: UnlistenFn | null = null;
const error = ref("");
/** Unique per spectrum: the strip and the station home can both show one. */
const bodyId = useId();

/** A theme token's value, as CSS gives it. */
const token = (name: string) => getComputedStyle(document.documentElement).getPropertyValue(`--${name}`).trim();
/** A theme token's red, green and blue, read through a canvas so any CSS form works. */
function channels(name: string): [number, number, number] {
  const value = token(name);
  const c = document.createElement("canvas").getContext("2d");
  if (!c || !value) return [0, 0, 0];
  c.fillStyle = value;
  const hex = c.fillStyle.replace("#", "");
  return [0, 2, 4].map((i) => parseInt(hex.slice(i, i + 2), 16) || 0) as [number, number, number];
}
const level = (db: number) => Math.min(1, Math.max(0, (db - DB_MIN) / (DB_MAX - DB_MIN)));

function fit(canvas: HTMLCanvasElement) {
  const w = Math.max(1, Math.round(canvas.clientWidth * devicePixelRatio));
  const h = Math.max(1, Math.round(canvas.clientHeight * devicePixelRatio));
  if (canvas.width !== w || canvas.height !== h) {
    canvas.width = w;
    canvas.height = h;
  }
}

function drawSpectrum(f: Frame) {
  const canvas = spectrumCanvas.value;
  const ctx = canvas?.getContext("2d");
  if (!canvas || !ctx) return;
  fit(canvas);
  const { width: w, height: h } = canvas;
  ctx.clearRect(0, 0, w, h);
  ctx.strokeStyle = token("border");
  ctx.lineWidth = 1;
  for (let db = DB_MIN; db <= DB_MAX; db += 20) {
    const y = h - level(db) * h;
    ctx.beginPath();
    ctx.moveTo(0, y);
    ctx.lineTo(w, y);
    ctx.stroke();
  }
  ctx.strokeStyle = token("accent");
  ctx.lineWidth = Math.max(1, devicePixelRatio);
  ctx.beginPath();
  f.bins.forEach((db, i) => {
    const x = (i / (f.bins.length - 1)) * w;
    const y = h - level(db) * h;
    if (i === 0) ctx.moveTo(x, y);
    else ctx.lineTo(x, y);
  });
  ctx.stroke();
}

function drawWaterfall() {
  const canvas = waterfallCanvas.value;
  const ctx = canvas?.getContext("2d");
  if (!canvas || !ctx || !rows.length) return;
  const width = rows[0].length;
  if (canvas.width !== width || canvas.height !== ROWS) {
    canvas.width = width;
    canvas.height = ROWS;
  }
  const lo = channels("waterfall-low"), hi = channels("waterfall-high");
  const img = ctx.createImageData(width, ROWS);
  rows.forEach((row, y) => {
    for (let x = 0; x < width; x++) {
      const v = level(row[x] ?? DB_MIN);
      const o = (y * width + x) * 4;
      img.data[o] = lo[0] + (hi[0] - lo[0]) * v;
      img.data[o + 1] = lo[1] + (hi[1] - lo[1]) * v;
      img.data[o + 2] = lo[2] + (hi[2] - lo[2]) * v;
      img.data[o + 3] = 255;
    }
  });
  ctx.putImageData(img, 0, 0);
}

function onFrame(f: Frame | null) {
  if (!active.value) return;
  if (!f) {
    drawing.value = false;
    rows.length = 0;
    clear();
    return;
  }
  drawing.value = true;
  rows.unshift(Float32Array.from(f.bins));
  if (rows.length > ROWS) rows.length = ROWS;
  const now = performance.now();
  if (reduceMotion.value && now - lastDraw < 1000) return;
  lastDraw = now;
  drawSpectrum(f);
  drawWaterfall();
}

function clear() {
  for (const c of [spectrumCanvas.value, waterfallCanvas.value]) c?.getContext("2d")?.clearRect(0, 0, c.width, c.height);
}

/** This spectrum's claim on the core's forwarding (spectrumDemand.ts). */
const me = Symbol("spectrum");
async function setForwarding(on: boolean) {
  error.value = await claimSpectrum(me, on);
}

watch(active, async (on) => {
  void setForwarding(on);
  if (!on) {
    rows.length = 0;
    drawing.value = false;
    clear();
  } else {
    await nextTick();
  }
});

const onVisibility = () => (pageVisible.value = document.visibilityState === "visible");
const onReduced = (e: MediaQueryListEvent) => (reduceMotion.value = e.matches);
onMounted(async () => {
  document.addEventListener("visibilitychange", onVisibility);
  reduced.addEventListener("change", onReduced);
  unlisten = await listen<Frame | null>("mc://spectrum", (e) => onFrame(e.payload));
  if (active.value) void setForwarding(true);
});
onUnmounted(() => {
  document.removeEventListener("visibilitychange", onVisibility);
  reduced.removeEventListener("change", onReduced);
  unlisten?.();
  void setForwarding(false);
});

/** Axis labels, drawn inside the canvases: kHz along the bottom, dB up the side. */
const AXIS_KHZ = ["0", "1k", "2k", "3k", "4k"];
const AXIS_DB = [-20, -70, -120];
</script>

<template>
  <div :class="['waterfall', { open, large }]">
    <div class="head">
      <span class="label">{{ t("strip.spectrum") }}</span>
      <button class="quiet wf-toggle small" :aria-expanded="open" :aria-controls="bodyId" @click="open = !open">
        {{ t(open ? "waterfall.hide" : "waterfall.show") }}
      </button>
    </div>
    <div v-if="open" :id="bodyId" class="body">
      <p v-if="!up" class="muted small wf-status" role="status">{{ t("waterfall.unavailable", { link: t(`telemetry.link.${link}`) }) }}</p>
      <p v-if="!up" class="muted small wf-how">{{ t(link === "unconfigured" ? "waterfall.howUnconfigured" : "waterfall.howLost") }}</p>
      <div v-else class="panels" :data-drawing="String(drawing)">
        <figure class="panel" :title="t('waterfall.spectrumAxis')">
          <canvas ref="spectrumCanvas" class="spectrum-canvas" role="img" :aria-label="t('waterfall.spectrumAria')"></canvas>
          <span class="y mono" aria-hidden="true"><span v-for="d in AXIS_DB" :key="d">{{ d }}</span></span>
          <span class="x mono" aria-hidden="true"><span v-for="hz in AXIS_KHZ" :key="hz">{{ hz }}</span></span>
        </figure>
        <figure class="panel" :title="t('waterfall.waterfallAxis')">
          <canvas ref="waterfallCanvas" class="waterfall-canvas" role="img" :aria-label="t('waterfall.waterfallAria')"></canvas>
          <span class="x mono" aria-hidden="true"><span v-for="hz in AXIS_KHZ" :key="hz">{{ hz }}</span></span>
        </figure>
      </div>
      <p v-if="error" class="err small" role="alert">{{ error }}</p>
    </div>
    <p v-else class="muted small closed-note" :title="up ? undefined : t(link === 'unconfigured' ? 'waterfall.howUnconfigured' : 'waterfall.howLost')">{{ up ? t("waterfall.closed") : t("strip.telemetryUnavailable") }}</p>
  </div>
</template>

<style scoped>
/* Compact, for the 60 px station strip: the label and Show/Hide on the left,
   then the spectrum and the waterfall, 40 px tall, axis labels drawn inside. */
.waterfall { display: flex; align-items: center; gap: var(--space); min-width: 0; height: 100%; }
.head { display: flex; flex-direction: column; align-items: flex-start; gap: 2px; }
.waterfall:not(.open) { flex-direction: row; }
.closed-note, .wf-status { margin: 0; white-space: nowrap; }
.panels { display: flex; gap: 6px; }
.panel { position: relative; margin: 0; width: 200px; height: 44px; }
canvas { width: 100%; height: 100%; display: block; background: var(--canvas); border: 1px solid var(--border); border-radius: 4px; box-sizing: border-box; }
.waterfall-canvas { image-rendering: pixelated; }
/* The station home's form: the panels stacked, the full width of the card. */
.waterfall.large { flex-direction: column; align-items: stretch; height: auto; }
.waterfall.large .head { flex-direction: row; align-items: center; justify-content: space-between; }
.waterfall.large .panels { flex-direction: column; gap: 8px; }
.waterfall.large .panel { width: 100%; height: 120px; }
.waterfall.large .panel + .panel { height: 180px; }
.waterfall.large .x, .waterfall.large .y { font-size: 10px; }
.x, .y { position: absolute; pointer-events: none; font-size: 9px; line-height: 1; color: var(--text-2); opacity: 0.85; }
.x { left: 3px; right: 3px; bottom: 2px; display: flex; justify-content: space-between; }
.y { top: 2px; bottom: 12px; left: 3px; display: flex; flex-direction: column; justify-content: space-between; }
</style>

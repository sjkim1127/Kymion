import init, { KymionEngine } from "./pkg/kymion_wasm.js";

await init();
const PPQ = 480, GRID = 240, KEY_H = 14, PX_PER_TICK = 0.12, LOW = 36, HIGH = 96, KEYS_W = 44;
let eng = new KymionEngine("Untitled");
let sel = null, ctx = null, src = null;
const $ = (id) => document.getElementById(id);
const proj = () => JSON.parse(eng.project_json());

// Single entry point for GUI, console and external agents (window.kymion).
function apply(cmd) {
  try {
    const out = eng.apply(typeof cmd === "string" ? cmd : JSON.stringify(cmd));
    log(out);
    refresh();
    return JSON.parse(out);
  } catch (e) {
    log("ERROR: " + e);
    return { ok: false, error: String(e) };
  }
}
const log = (s) => {
  $("log").textContent = s + "\n" + $("log").textContent.slice(0, 600);
};

window.kymion = {
  apply,
  project: proj,
  schema: () => JSON.parse(KymionEngine.schema()),
  analysis: () => JSON.parse(eng.analysis_json()),
  importMidi: (bytes) => {
    eng = KymionEngine.import_midi(bytes);
    refresh();
  },
  exportMidi: () => eng.export_midi(),
};

function refresh() {
  const p = proj();
  if (sel === null || !p.tracks.find((t) => t.id === sel)) {
    sel = p.tracks[0]?.id ?? null;
  }
  $("tempo").value = Math.round(p.tempo);

  // Render Tracks
  $("tracks").innerHTML = "";
  for (const t of p.tracks) {
    const d = document.createElement("div");
    d.className = "trk" + (t.id === sel ? " sel" : "");
    const fxCount = t.fx?.length ? ` [${t.fx.length} FX]` : "";
    d.innerHTML = `<span>${t.kind.type === "vocal" ? "🎤" : "🎹"} ${t.name}${fxCount}</span><span class="spacer" style="flex:1"></span>
      <input type="range" min="0" max="1.5" step="0.05" value="${t.gain}" title="gain">
      <input type="checkbox" ${t.mute ? "checked" : ""} title="mute">`;
    d.onclick = () => { sel = t.id; refresh(); };
    const [g, m] = d.querySelectorAll("input");
    g.onclick = m.onclick = (e) => e.stopPropagation();
    g.oninput = () => apply({ cmd: "set_mix", track: t.id, gain: +g.value });
    m.onchange = () => apply({ cmd: "set_mix", track: t.id, mute: m.checked });
    $("tracks").append(d);
  }

  renderFxRacks(p);
  renderExpr(p);
  draw(p);
}

function renderFxList(container, fxList, trackId) {
  container.innerHTML = "";
  if (!fxList || fxList.length === 0) {
    container.innerHTML = `<div style="color:#666;font-size:11px;padding:4px 0;">No effects inserted.</div>`;
    return;
  }

  for (const fx of fxList) {
    const card = document.createElement("div");
    card.className = "fx-card";
    const typeName = fx.kind.type.toUpperCase();

    let controlsHtml = "";
    if (fx.kind.type === "reverb") {
      controlsHtml = `
        <div class="row"><span>Room</span><input type="range" data-key="room_size" min="0" max="1" step="0.05" value="${fx.kind.room_size}"></div>
        <div class="row"><span>Damp</span><input type="range" data-key="damping" min="0" max="1" step="0.05" value="${fx.kind.damping}"></div>
        <div class="row"><span>Mix</span><input type="range" data-key="mix" min="0" max="1" step="0.05" value="${fx.kind.mix}"></div>`;
    } else if (fx.kind.type === "delay") {
      controlsHtml = `
        <div class="row"><span>Time (ms)</span><input type="range" data-key="time_ms" min="20" max="1000" step="10" value="${fx.kind.time_ms}"></div>
        <div class="row"><span>Feedback</span><input type="range" data-key="feedback" min="0" max="0.9" step="0.05" value="${fx.kind.feedback}"></div>
        <div class="row"><span>Mix</span><input type="range" data-key="mix" min="0" max="1" step="0.05" value="${fx.kind.mix}"></div>`;
    } else if (fx.kind.type === "eq") {
      controlsHtml = `
        <div class="row"><span>Low (dB)</span><input type="range" data-key="low_gain" min="-12" max="12" step="0.5" value="${fx.kind.low_gain}"></div>
        <div class="row"><span>Mid (dB)</span><input type="range" data-key="mid_gain" min="-12" max="12" step="0.5" value="${fx.kind.mid_gain}"></div>
        <div class="row"><span>High (dB)</span><input type="range" data-key="high_gain" min="-12" max="12" step="0.5" value="${fx.kind.high_gain}"></div>`;
    } else if (fx.kind.type === "compressor") {
      controlsHtml = `
        <div class="row"><span>Thresh</span><input type="range" data-key="threshold_db" min="-40" max="0" step="1" value="${fx.kind.threshold_db}"></div>
        <div class="row"><span>Ratio</span><input type="range" data-key="ratio" min="1" max="16" step="0.5" value="${fx.kind.ratio}"></div>`;
    }

    card.innerHTML = `
      <div class="fx-header">
        <label><input type="checkbox" class="fx-toggle" ${fx.enabled ? "checked" : ""}> ${typeName}</label>
        <button class="del-btn" title="Delete effect">×</button>
      </div>
      ${controlsHtml}
    `;

    // Toggle on/off
    card.querySelector(".fx-toggle").onchange = (e) => {
      apply({ cmd: "set_fx", track: trackId, fx: fx.id, enabled: e.target.checked });
    };

    // Delete
    card.querySelector(".del-btn").onclick = () => {
      apply({ cmd: "remove_fx", track: trackId, fx: fx.id });
    };

    // Sliders
    for (const input of card.querySelectorAll("input[type=range]")) {
      input.onchange = (e) => {
        const key = e.target.getAttribute("data-key");
        const updatedKind = { ...fx.kind, [key]: +e.target.value };
        apply({ cmd: "set_fx", track: trackId, fx: fx.id, kind: updatedKind });
      };
    }

    container.append(card);
  }
}

function renderFxRacks(p) {
  const currentTrack = p.tracks.find((t) => t.id === sel);
  const trackFxBox = $("fx-rack");
  if (!currentTrack) {
    trackFxBox.innerHTML = `<div style="color:#666;font-size:11px;">Select a track to view FX.</div>`;
  } else {
    renderFxList(trackFxBox, currentTrack.fx, currentTrack.id);
  }
  renderFxList($("master-fx-rack"), p.master_fx, null);
}

function renderExpr(p) {
  const t = p.tracks.find((x) => x.id === sel), box = $("expr");
  box.innerHTML = "";
  if (!t || t.kind.type !== "vocal") {
    box.innerHTML = `<div style="color:#666;font-size:11px;">Select a vocal track for expression controls.</div>`;
    return;
  }
  const defs = [
    ["vibrato_depth", 0, 100, 1],
    ["vibrato_rate", 1, 10, 0.1],
    ["vibrato_delay", 0, 1, 0.05],
    ["portamento", 0, 0.3, 0.01],
    ["breath", 0, 1, 0.05],
    ["formant_shift", -6, 6, 0.5]
  ];
  for (const [k, lo, hi, st] of defs) {
    const r = document.createElement("div");
    r.className = "row";
    r.innerHTML = `<span>${k}</span><input type="range" min="${lo}" max="${hi}" step="${st}" value="${t.expression[k]}">`;
    r.querySelector("input").onchange = (e) => apply({ cmd: "set_expression", track: t.id, expression: { ...t.expression, [k]: +e.target.value } });
    box.append(r);
  }
}

// ---- Piano Roll ----
const cv = $("roll"), g = cv.getContext("2d");
function draw(p = proj()) {
  const dpr = devicePixelRatio || 1, r = cv.getBoundingClientRect();
  cv.width = r.width * dpr; cv.height = r.height * dpr; g.setTransform(dpr, 0, 0, dpr, 0, 0);
  g.fillStyle = "#0e0e11"; g.fillRect(0, 0, r.width, r.height);
  for (let n = LOW; n <= HIGH; n++) {
    const y = (HIGH - n) * KEY_H, black = [1, 3, 6, 8, 10].includes(n % 12);
    g.fillStyle = black ? "#121216" : "#17171d"; g.fillRect(KEYS_W, y, r.width, KEY_H);
    g.fillStyle = black ? "#222" : "#ddd"; g.fillRect(0, y, KEYS_W, KEY_H - 1);
    if (n % 12 === 0) { g.fillStyle = "#444"; g.font = "9px sans-serif"; g.fillText("C" + (n / 12 - 1), 6, y + 10); }
  }
  for (let t = 0; t * PX_PER_TICK < r.width; t += PPQ) {
    g.strokeStyle = t % (PPQ * 4) === 0 ? "#33333e" : "#1f1f26";
    const x = KEYS_W + t * PX_PER_TICK; g.beginPath(); g.moveTo(x, 0); g.lineTo(x, r.height); g.stroke();
  }
  for (const t of p.tracks) for (const n of t.notes) {
    const x = KEYS_W + n.start * PX_PER_TICK, y = (HIGH - n.pitch) * KEY_H, w = n.duration * PX_PER_TICK;
    g.globalAlpha = t.id === sel ? 1 : 0.3; g.fillStyle = t.kind.type === "vocal" ? "#d4af37" : "#4aa3c9";
    g.fillRect(x, y + 1, Math.max(3, w - 1), KEY_H - 2);
    if (n.lyric) { g.fillStyle = "#000"; g.font = "10px sans-serif"; g.fillText(n.lyric, x + 3, y + 10); }
    g.globalAlpha = 1;
  }
}

function hit(e, p) {
  const r = cv.getBoundingClientRect(), x = e.clientX - r.left, y = e.clientY - r.top;
  const tick = Math.max(0, Math.floor((x - KEYS_W) / PX_PER_TICK / GRID) * GRID), pitch = HIGH - Math.floor(y / KEY_H);
  const t = p.tracks.find((q) => q.id === sel);
  const note = t?.notes.find((n) => pitch === n.pitch && tick + 1 >= n.start && tick < n.start + n.duration);
  return { tick, pitch, t, note };
}

cv.onclick = (e) => {
  const p = proj(), { tick, pitch, t, note } = hit(e, p);
  if (!t || e.clientX - cv.getBoundingClientRect().left < KEYS_W) return;
  if (note) { if (e.shiftKey) apply({ cmd: "remove_note", track: t.id, note: note.id }); return; }
  apply({ cmd: "add_note", track: t.id, start: tick, duration: PPQ / 2, pitch, lyric: t.kind.type === "vocal" ? "la" : null });
};
cv.ondblclick = (e) => {
  const p = proj(), { t, note } = hit(e, p);
  if (t && note) {
    const l = prompt("Lyric syllable", note.lyric ?? "la");
    if (l != null) apply({ cmd: "set_lyrics", track: t.id, lyrics: t.notes.map((n) => (n.id === note.id ? l : n.lyric ?? "la")) });
  }
};
cv.oncontextmenu = (e) => {
  e.preventDefault();
  const p = proj(), { t, note } = hit(e, p);
  if (t && note) apply({ cmd: "remove_note", track: t.id, note: note.id });
};
addEventListener("resize", () => draw());

// ---- Transport ----
function stop() { try { src?.stop(); } catch {} src = null; }
$("play").onclick = async () => {
  stop();
  ctx ??= new AudioContext({ sampleRate: eng.sample_rate() });
  await ctx.resume();
  const data = eng.render(), n = data.length / 2;
  if (!n) return;
  const buf = ctx.createBuffer(2, n, eng.sample_rate());
  const L = buf.getChannelData(0), R = buf.getChannelData(1);
  for (let i = 0; i < n; i++) { L[i] = data[2 * i]; R[i] = data[2 * i + 1]; }
  src = ctx.createBufferSource(); src.buffer = buf; src.connect(ctx.destination); src.start();
};
$("stop").onclick = stop;
$("undo").onclick = () => { try { eng.undo(); refresh(); } catch (e) { log(String(e)); } };
$("redo").onclick = () => { try { eng.redo(); refresh(); } catch (e) { log(String(e)); } };
$("tempo").onchange = (e) => apply({ cmd: "set_tempo", bpm: +e.target.value });

// Track Actions
$("add-inst").onclick = () => apply({ cmd: "add_track", name: "Instrument", kind: { type: "instrument", waveform: "pluck" } });
$("add-vocal").onclick = () => apply({ cmd: "add_track", name: "Vocal", kind: { type: "vocal", voice: "default" } });

// FX Actions
$("add-fx-eq").onclick = () => {
  if (sel != null) apply({ cmd: "add_fx", track: sel, kind: { type: "eq", low_gain: 2, mid_gain: -1, mid_freq: 1000, mid_q: 1, high_gain: 2 } });
};
$("add-fx-delay").onclick = () => {
  if (sel != null) apply({ cmd: "add_fx", track: sel, kind: { type: "delay", time_ms: 250, feedback: 0.45, mix: 0.35, ping_pong: true } });
};
$("add-fx-reverb").onclick = () => {
  if (sel != null) apply({ cmd: "add_fx", track: sel, kind: { type: "reverb", room_size: 0.75, damping: 0.25, width: 1.0, mix: 0.35 } });
};

// Master FX Actions
$("add-master-fx-eq").onclick = () => {
  apply({ cmd: "add_fx", track: null, kind: { type: "eq", low_gain: 1.5, mid_gain: 0, mid_freq: 1200, mid_q: 1, high_gain: 2.0 } });
};
$("add-master-fx-comp").onclick = () => {
  apply({ cmd: "add_fx", track: null, kind: { type: "compressor", threshold_db: -12, ratio: 4, attack_ms: 15, release_ms: 100, makeup_db: 2 } });
};

// Console
$("run").onclick = () => {
  try { const v = JSON.parse($("cmd").value); (Array.isArray(v) ? v : [v]).forEach(apply); } catch (e) { log("ERROR: " + e); }
};

// Downloads & File Handling
const download = (name, data, type) => {
  const a = document.createElement("a");
  a.href = URL.createObjectURL(new Blob([data], { type }));
  a.download = name;
  a.click();
};

$("wav").onclick = () => download("kymion.wav", eng.wav(), "audio/wav");
$("save").onclick = () => download("project.kymion.json", eng.project_json(), "application/json");
$("load").onclick = () => $("file").click();
$("file").onchange = async (e) => {
  try { eng = KymionEngine.load(await e.target.files[0].text()); refresh(); } catch (err) { log("ERROR: " + err); }
};

// MIDI Export & Import
$("export-midi").onclick = () => {
  try {
    const bytes = eng.export_midi();
    download("project.mid", bytes, "audio/midi");
    log("Exported MIDI file successfully.");
  } catch (err) {
    log("MIDI Export Error: " + err);
  }
};
$("import-midi").onclick = () => $("midi-file").click();
$("midi-file").onchange = async (e) => {
  try {
    const file = e.target.files[0];
    if (!file) return;
    const arrayBuffer = await file.arrayBuffer();
    eng = KymionEngine.import_midi(new Uint8Array(arrayBuffer));
    refresh();
    log(`Imported MIDI "${file.name}" successfully.`);
  } catch (err) {
    log("MIDI Import Error: " + err);
  }
};

// Drag & Drop .mid or .json directly onto the canvas/window
window.addEventListener("dragover", (e) => e.preventDefault());
window.addEventListener("drop", async (e) => {
  e.preventDefault();
  const file = e.dataTransfer?.files[0];
  if (!file) return;
  if (file.name.endsWith(".mid") || file.name.endsWith(".midi")) {
    const buf = await file.arrayBuffer();
    eng = KymionEngine.import_midi(new Uint8Array(buf));
    refresh();
    log(`Dropped and loaded MIDI "${file.name}".`);
  } else if (file.name.endsWith(".json")) {
    eng = KymionEngine.load(await file.text());
    refresh();
    log(`Dropped and loaded project "${file.name}".`);
  }
});

// Initial demo project
apply({ cmd: "add_track", name: "Grand Piano", kind: { type: "instrument", waveform: "pluck" } });
apply({ cmd: "add_track", name: "Lead Vocal", kind: { type: "vocal", voice: "default" } });
apply({ cmd: "add_fx", track: 1, kind: { type: "reverb", room_size: 0.65, damping: 0.3, width: 1.0, mix: 0.25 } });
apply({ cmd: "add_fx", track: 2, kind: { type: "delay", time_ms: 220, feedback: 0.4, mix: 0.25, ping_pong: true } });

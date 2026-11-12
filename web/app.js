import init, { KymionEngine } from "./pkg/kymion_wasm.js";

await init();

// ───────────────────────── constants & state ─────────────────────────
const PPQ = 480, RULER_H = 26, VEL_H = 72, LANE_H = 60, KEY_H = 14, LOW = 24, HIGH = 108;
const $ = (id) => document.getElementById(id);
let eng = new KymionEngine("Untitled");
let P = null;                     // cached light project (no audio payloads)
let view = "arrange", tool = "draw";
let ppt = 0.12, scrollX = 0, scrollY = 0;   // pixels per tick, scroll offsets
let snap = 240;
let sel = null;                   // selected track id
let selNotes = new Set(), selClip = null;
let playhead = 0, playing = false, metro = false;
let ctx = null, srcNode = null, t0 = 0, startTick = 0, cache = null, dirty = true;
let workletNode = null, streamPos = 0, workletBuffered = 0, lastMeterUpdate = 0;
let drag = null, clipboard = [];
const peaksCache = new Map();
let activePreviewKey = null;
let activePreviewSource = null;

const NOTE_NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
const pitchName = (p) => `${NOTE_NAMES[p % 12]}${Math.floor(p / 12) - 1}`;
const yToPitch = (y) => HIGH - Math.floor((y - RULER_H + scrollY) / KEY_H);

async function playNotePreview(trackId, pitch, duration = 0.35, velocity = 100) {
  if (trackId == null) return;
  try {
    ctx ??= new AudioContext({ sampleRate: eng.sample_rate() });
    if (ctx.state === "suspended") await ctx.resume();

    const data = eng.preview_note(trackId, pitch, duration, velocity);
    if (!data || data.length === 0) return;

    const numFrames = data.length / 2;
    const sr = eng.sample_rate();
    const buf = ctx.createBuffer(2, numFrames, sr);
    const L = buf.getChannelData(0);
    const R = buf.getChannelData(1);
    for (let i = 0; i < numFrames; i++) {
      L[i] = data[2 * i];
      R[i] = data[2 * i + 1];
    }

    if (activePreviewSource) {
      try { activePreviewSource.stop(); } catch {}
      activePreviewSource = null;
    }

    const src = ctx.createBufferSource();
    src.buffer = buf;
    src.connect(ctx.destination);
    src.start();
    activePreviewSource = src;
    src.onended = () => {
      if (activePreviewSource === src) activePreviewSource = null;
    };
  } catch (err) {
    console.warn("preview_note failed:", err);
  }
}

const light = () => JSON.parse(eng.project_json_light());
const track = () => P?.tracks.find((t) => t.id === sel);
const spt = () => 60 / (P.tempo * PPQ);                    // seconds per tick
const gutter = () => (view === "roll" ? 44 : 120);
const tickToX = (t) => gutter() + (t - scrollX) * ppt;
const xToTick = (x) => (x - gutter()) / ppt + scrollX;
const snapDelta = (d) => (snap <= 1 ? Math.round(d) : Math.round(d / snap) * snap);
const snapFloor = (t) => Math.max(0, snap <= 1 ? Math.round(t) : Math.floor(t / snap) * snap);
const snapRound = (t) => Math.max(0, snap <= 1 ? Math.round(t) : Math.round(t / snap) * snap);
const clipLen = (c) => {
  if (c.duration_ticks) return c.duration_ticks;
  const a = P?.assets?.find((x) => x.id === c.asset);
  return a ? Math.max(0, Math.round(a.duration_sec * P.tempo * PPQ / 60) - (c.offset_ticks || 0)) : 0;
};

// ───────────────────────── command entry point ─────────────────────────
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
const log = (s) => { $("log").textContent = s + "\n" + $("log").textContent.slice(0, 600); };

window.kymion = {
  apply,
  project: () => light(),
  schema: () => JSON.parse(KymionEngine.schema()),
  analysis: () => JSON.parse(eng.analysis_json()),
  importMidi: (bytes) => { eng = KymionEngine.import_midi(bytes); refresh(); },
  exportMidi: () => eng.export_midi(),
  play: () => play(), stop: () => stopAll(), get playhead() { return curTick(); },
  meters: () => JSON.parse(eng.meter_levels(streamPos, 2048)),
  previewNote: (trackId, pitch, duration, velocity) => playNotePreview(trackId, pitch, duration, velocity),
};

// ───────────────────────── refresh / side panels ─────────────────────────
function refresh() {
  P = light();
  if (sel === null || !P.tracks.find((t) => t.id === sel)) sel = P.tracks[0]?.id ?? null;
  const ids = new Set(track()?.notes.map((n) => n.id) ?? []);
  selNotes = new Set([...selNotes].filter((i) => ids.has(i)));

  $("tempo").value = Math.round(P.tempo);
  $("ts-num").value = P.time_sig_num;
  $("ts-den").value = P.time_sig_den;
  $("loop").classList.toggle("active", !!P.loop_region);
  $("metro").classList.toggle("active", metro);
  $("view-arrange").classList.toggle("active", view === "arrange");
  $("view-roll").classList.toggle("active", view === "roll");
  $("tool-draw").classList.toggle("active", tool === "draw");
  $("tool-select").classList.toggle("active", tool === "select");

  renderTracks();
  renderInstrumentPanel();
  renderFxRacks();
  renderExpr();
  dirty = true;
  if (playing && workletNode) {
    // Prime new audio forward from current playhead so edits are heard seamlessly
    const sr = eng.sample_rate();
    streamPos = Math.round(playhead * spt() * sr);
    workletNode.port.postMessage({ type: "clear" });
    workletBuffered = 0;
    feedWorklet();
  }
  draw();
}

const panText = (p) => (p === 0 ? "C" : p < 0 ? `L${Math.round(-p * 100)}` : `R${Math.round(p * 100)}`);
let renderedTrackKey = "";

function renderTracks() {
  const box = $("tracks");
  const trackKey = P.tracks.map((t) => t.id).join(",");

  if (trackKey === renderedTrackKey && box.children.length === P.tracks.length) {
    // Fast path: in-place DOM update (avoids layout thrashing and GC overhead)
    P.tracks.forEach((t, i) => {
      const d = box.children[i];
      if (!d) return;
      d.className = "trk" + (t.id === sel ? " sel" : "");
      const sfTag = t.kind.soundfont ? " [SF2]" : "";
      const nameEl = d.querySelector(".tname");
      if (nameEl) nameEl.textContent = `${t.name}${sfTag}${t.fx.length ? ` [${t.fx.length} FX]` : ""}`;

      const muteBtn = d.querySelector('[data-a="mute"]');
      if (muteBtn) muteBtn.className = "ms " + (t.mute ? "on-mute" : "");

      const soloBtn = d.querySelector('[data-a="solo"]');
      if (soloBtn) soloBtn.className = "ms " + (t.solo ? "on-solo" : "");

      const vol = d.querySelector(".vol-slider");
      if (vol && document.activeElement !== vol) vol.value = t.gain;

      const pan = d.querySelector(".pan-slider");
      const panLbl = d.querySelector(".pan-lbl");
      if (pan && document.activeElement !== pan) pan.value = t.pan;
      if (panLbl) panLbl.textContent = panText(t.pan);
    });
    return;
  }

  renderedTrackKey = trackKey;
  box.innerHTML = "";
  for (const t of P.tracks) {
    const sfTag = t.kind.soundfont ? " [SF2]" : "";
    const d = document.createElement("div");
    d.className = "trk" + (t.id === sel ? " sel" : "");
    d.dataset.tid = t.id;
    d.innerHTML = `
      <div style="display:flex;align-items:center;width:100%;gap:4px">
        <span class="tname" title="Double-click to rename">${t.name}${sfTag}${t.fx.length ? ` [${t.fx.length} FX]` : ""}</span>
        <span class="spacer" style="flex:1"></span>
        <button class="ms ${t.mute ? "on-mute" : ""}" data-a="mute" title="Mute">M</button>
        <button class="ms ${t.solo ? "on-solo" : ""}" data-a="solo" title="Solo">S</button>
        <button class="ms" data-a="del" title="Delete track">×</button>
      </div>
      <div class="trk-controls">
        <div class="trk-row">
          <span class="lbl">Vol</span>
          <input type="range" class="vol-slider" min="0" max="1.5" step="0.05" value="${t.gain}" title="Volume (${Math.round(t.gain * 100)}%)">
          <div class="trk-meter-wrap"><div class="trk-meter-fill" data-meter-tid="${t.id}"></div></div>
        </div>
        <div class="trk-row">
          <span class="lbl">Pan</span>
          <input type="range" class="pan-slider" min="-1" max="1" step="0.05" value="${t.pan}" title="Pan">
          <span class="pan-lbl">${panText(t.pan)}</span>
        </div>
      </div>`;
    d.onclick = () => { sel = t.id; selNotes.clear(); selClip = null; refresh(); };
    d.querySelector(".tname").ondblclick = (e) => {
      e.stopPropagation();
      const n = prompt("Track name", t.name);
      if (n) apply({ cmd: "rename_track", track: t.id, name: n });
    };
    d.querySelectorAll("button").forEach((b) => (b.onclick = (e) => {
      e.stopPropagation();
      const a = b.dataset.a;
      if (a === "mute") apply({ cmd: "set_mix", track: t.id, mute: !t.mute });
      else if (a === "solo") apply({ cmd: "set_mix", track: t.id, solo: !t.solo });
      else if (confirm(`Delete track "${t.name}"?`)) apply({ cmd: "remove_track", track: t.id });
    }));
    const vol = d.querySelector(".vol-slider");
    vol.onclick = (e) => e.stopPropagation();
    vol.oninput = (e) => {
      e.stopPropagation();
      apply({ cmd: "set_mix", track: t.id, gain: +vol.value });
    };
    const pan = d.querySelector(".pan-slider");
    const panLbl = d.querySelector(".pan-lbl");
    pan.onclick = (e) => e.stopPropagation();
    pan.oninput = (e) => {
      e.stopPropagation();
      panLbl.textContent = panText(+pan.value);
      apply({ cmd: "set_mix", track: t.id, pan: +pan.value });
    };
    box.append(d);
  }
}

function renderFxList(container, fxList, trackId) {
  container.innerHTML = "";
  if (!fxList || fxList.length === 0) {
    container.innerHTML = `<div class="empty">No effects inserted.</div>`;
    return;
  }
  const sliders = {
    reverb: [["Room", "room_size", 0, 1, 0.05], ["Damp", "damping", 0, 1, 0.05], ["Mix", "mix", 0, 1, 0.05]],
    delay: [["Time ms", "time_ms", 20, 1000, 10], ["Feedback", "feedback", 0, 0.9, 0.05], ["Mix", "mix", 0, 1, 0.05]],
    eq: [["Low dB", "low_gain", -12, 12, 0.5], ["Mid dB", "mid_gain", -12, 12, 0.5], ["High dB", "high_gain", -12, 12, 0.5]],
    compressor: [["Thresh", "threshold_db", -40, 0, 1], ["Ratio", "ratio", 1, 16, 0.5]],
  };
  for (const fx of fxList) {
    const card = document.createElement("div");
    card.className = "fx-card";
    const rows = (sliders[fx.kind.type] || []).map(([l, k, lo, hi, st]) =>
      `<div class="row"><span>${l}</span><input type="range" data-key="${k}" min="${lo}" max="${hi}" step="${st}" value="${fx.kind[k]}"></div>`).join("");
    card.innerHTML = `<div class="fx-header"><label><input type="checkbox" class="fx-toggle" ${fx.enabled ? "checked" : ""}> ${fx.kind.type.toUpperCase()}</label>
      <button class="del-btn" title="Delete effect">×</button></div>${rows}`;
    card.querySelector(".fx-toggle").onchange = (e) => apply({ cmd: "set_fx", track: trackId, fx: fx.id, enabled: e.target.checked });
    card.querySelector(".del-btn").onclick = () => apply({ cmd: "remove_fx", track: trackId, fx: fx.id });
    card.querySelectorAll("input[type=range]").forEach((i) => (i.onchange = (e) =>
      apply({ cmd: "set_fx", track: trackId, fx: fx.id, kind: { ...fx.kind, [e.target.dataset.key]: +e.target.value } })));
    container.append(card);
  }
}
function renderFxRacks() {
  const t = track();
  if (t) renderFxList($("fx-rack"), t.fx, t.id);
  else $("fx-rack").innerHTML = `<div class="empty">Select a track.</div>`;
  renderFxList($("master-fx-rack"), P.master_fx, null);
}

const esc = (s) => String(s).replace(/[&<>"']/g, (m) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[m]));

function renderInstrumentPanel() {
  const box = $("inst-panel");
  if (!box) return;
  const t = track();
  if (!t || t.kind.type !== "instrument") {
    box.innerHTML = `<div class="empty">Select an instrument track.</div>`;
    return;
  }

  const sf = t.kind.soundfont;
  const soundfonts = P.soundfonts || [];

  let html = `<div style="display:flex;flex-direction:column;gap:6px;padding:4px 0">`;

  // Source selection: Oscillator vs loaded SoundFonts
  html += `<div class="row"><label style="font-size:11px;width:60px">Source</label>`;
  html += `<select id="inst-source-sel" style="flex:1">`;
  html += `<option value="osc" ${!sf ? "selected" : ""}>Synth (Oscillator)</option>`;
  for (const s of soundfonts) {
    html += `<option value="sf_${s.id}" ${sf && sf.asset === s.id ? "selected" : ""}>SF2: ${esc(s.name)}</option>`;
  }
  html += `</select></div>`;

  if (!sf) {
    // Waveform selection
    html += `<div class="row"><label style="font-size:11px;width:60px">Waveform</label>`;
    html += `<select id="inst-wf-sel" style="flex:1">`;
    for (const wf of ["pluck", "saw", "square", "sine", "triangle"]) {
      html += `<option value="${wf}" ${t.kind.waveform === wf ? "selected" : ""}>${wf.toUpperCase()}</option>`;
    }
    html += `</select></div>`;

    const synth = t.kind.synth || {
      filter: "moog",
      cutoff_hz: 20000,
      resonance: 0,
      filter_env_amount: 0,
      attack: 0.005,
      decay: 0.1,
      sustain: 0.7,
      release: 0.2,
    };

    html += `<div class="synth-grid">`;
    html += `<div class="synth-field"><label>Filter</label>
      <select id="synth-filter" style="font-size:10px">
        <option value="moog" ${synth.filter === "moog" ? "selected" : ""}>Moog 24dB</option>
        <option value="lowpass" ${synth.filter === "lowpass" ? "selected" : ""}>SVF LPF 12dB</option>
        <option value="highpass" ${synth.filter === "highpass" ? "selected" : ""}>Highpass</option>
        <option value="bandpass" ${synth.filter === "bandpass" ? "selected" : ""}>Bandpass</option>
        <option value="none" ${synth.filter === "none" ? "selected" : ""}>Off</option>
      </select></div>`;

    html += `<div class="synth-field"><label>Cutoff <span id="lbl-cutoff">${Math.round(synth.cutoff_hz)}Hz</span></label>
      <input type="range" id="synth-cutoff" min="20" max="20000" step="20" value="${synth.cutoff_hz}"></div>`;

    html += `<div class="synth-field"><label>Resonance <span id="lbl-reso">${(synth.resonance * 100).toFixed(0)}%</span></label>
      <input type="range" id="synth-reso" min="0" max="0.95" step="0.01" value="${synth.resonance}"></div>`;

    html += `<div class="synth-field"><label>Env Mod <span id="lbl-env-amt">${(synth.filter_env_amount * 100).toFixed(0)}%</span></label>
      <input type="range" id="synth-env-amt" min="-1" max="1" step="0.05" value="${synth.filter_env_amount}"></div>`;

    html += `<div class="synth-field"><label>Attack <span id="lbl-att">${(synth.attack * 1000).toFixed(0)}ms</span></label>
      <input type="range" id="synth-att" min="0.001" max="1.0" step="0.005" value="${synth.attack}"></div>`;

    html += `<div class="synth-field"><label>Decay <span id="lbl-dec">${(synth.decay * 1000).toFixed(0)}ms</span></label>
      <input type="range" id="synth-dec" min="0.01" max="2.0" step="0.01" value="${synth.decay}"></div>`;

    html += `<div class="synth-field"><label>Sustain <span id="lbl-sus">${(synth.sustain * 100).toFixed(0)}%</span></label>
      <input type="range" id="synth-sus" min="0" max="1.0" step="0.01" value="${synth.sustain}"></div>`;

    html += `<div class="synth-field"><label>Release <span id="lbl-rel">${(synth.release * 1000).toFixed(0)}ms</span></label>
      <input type="range" id="synth-rel" min="0.01" max="2.5" step="0.01" value="${synth.release}"></div>`;

    html += `</div>`;
  } else {
    // Preset selection for SoundFont
    let presets = [];
    try {
      presets = JSON.parse(eng.soundfont_presets(sf.asset));
    } catch (_) {}

    html += `<div class="row"><label style="font-size:11px;width:60px">Preset</label>`;
    html += `<select id="inst-preset-sel" style="flex:1">`;
    if (presets.length === 0) {
      html += `<option value="0:0">Bank 0 - Preset 0</option>`;
    } else {
      for (const p of presets) {
        const val = `${p.bank}:${p.preset}`;
        const isSel = sf.bank === p.bank && sf.preset === p.preset;
        html += `<option value="${val}" ${isSel ? "selected" : ""}>${p.preset.toString().padStart(3, "0")}: ${esc(p.name)}</option>`;
      }
    }
    html += `</select></div>`;
  }

  html += `<div class="btn-row" style="margin-top:2px"><button id="btn-load-sf2" style="font-size:11px">+ Load .sf2 SoundFont</button></div>`;
  html += `</div>`;

  box.innerHTML = html;

  const srcSel = $("inst-source-sel");
  if (srcSel) {
    srcSel.onchange = (e) => {
      const v = e.target.value;
      if (v === "osc") {
        apply({ cmd: "set_track_soundfont", track: t.id, soundfont: null });
      } else if (v.startsWith("sf_")) {
        const assetId = parseInt(v.replace("sf_", ""), 10);
        let defaultBank = 0, defaultPreset = 0;
        try {
          const p = JSON.parse(eng.soundfont_presets(assetId));
          if (p.length > 0) { defaultBank = p[0].bank; defaultPreset = p[0].preset; }
        } catch (_) {}
        apply({
          cmd: "set_track_soundfont",
          track: t.id,
          soundfont: { asset: assetId, bank: defaultBank, preset: defaultPreset }
        });
      }
      refresh();
    };
  }

  const wfSel = $("inst-wf-sel");
  if (wfSel) {
    wfSel.onchange = (e) => {
      apply({ cmd: "set_track_waveform", track: t.id, waveform: e.target.value });
      refresh();
    };
  }

  const updateSynth = (key, val) => {
    const cur = t.kind.synth || {
      filter: "moog",
      cutoff_hz: 20000,
      resonance: 0,
      filter_env_amount: 0,
      attack: 0.005,
      decay: 0.1,
      sustain: 0.7,
      release: 0.2,
    };
    const s = { ...cur, [key]: val };
    apply({ cmd: "set_track_synth", track: t.id, synth: s });
  };

  if ($("synth-filter")) $("synth-filter").onchange = (e) => updateSynth("filter", e.target.value);
  if ($("synth-cutoff")) {
    $("synth-cutoff").oninput = (e) => {
      if ($("lbl-cutoff")) $("lbl-cutoff").textContent = Math.round(+e.target.value) + "Hz";
      updateSynth("cutoff_hz", +e.target.value);
    };
  }
  if ($("synth-reso")) {
    $("synth-reso").oninput = (e) => {
      if ($("lbl-reso")) $("lbl-reso").textContent = (+e.target.value * 100).toFixed(0) + "%";
      updateSynth("resonance", +e.target.value);
    };
  }
  if ($("synth-env-amt")) {
    $("synth-env-amt").oninput = (e) => {
      if ($("lbl-env-amt")) $("lbl-env-amt").textContent = (+e.target.value * 100).toFixed(0) + "%";
      updateSynth("filter_env_amount", +e.target.value);
    };
  }
  if ($("synth-att")) {
    $("synth-att").oninput = (e) => {
      if ($("lbl-att")) $("lbl-att").textContent = (+e.target.value * 1000).toFixed(0) + "ms";
      updateSynth("attack", +e.target.value);
    };
  }
  if ($("synth-dec")) {
    $("synth-dec").oninput = (e) => {
      if ($("lbl-dec")) $("lbl-dec").textContent = (+e.target.value * 1000).toFixed(0) + "ms";
      updateSynth("decay", +e.target.value);
    };
  }
  if ($("synth-sus")) {
    $("synth-sus").oninput = (e) => {
      if ($("lbl-sus")) $("lbl-sus").textContent = (+e.target.value * 100).toFixed(0) + "%";
      updateSynth("sustain", +e.target.value);
    };
  }
  if ($("synth-rel")) {
    $("synth-rel").oninput = (e) => {
      if ($("lbl-rel")) $("lbl-rel").textContent = (+e.target.value * 1000).toFixed(0) + "ms";
      updateSynth("release", +e.target.value);
    };
  }

  const preSel = $("inst-preset-sel");
  if (preSel) {
    preSel.onchange = (e) => {
      const [bank, preset] = e.target.value.split(":").map((x) => parseInt(x, 10));
      apply({
        cmd: "set_track_soundfont",
        track: t.id,
        soundfont: { asset: sf.asset, bank, preset }
      });
      refresh();
    };
  }

  const btnLoad = $("btn-load-sf2");
  if (btnLoad) {
    btnLoad.onclick = () => $("sf2-file").click();
  }
}

function renderExpr() {
  const t = track(), box = $("expr");
  box.innerHTML = "";
  if (!t || t.kind.type !== "vocal") { box.innerHTML = `<div class="empty">Select a vocal track.</div>`; return; }
  for (const [k, lo, hi, st] of [["vibrato_depth", 0, 100, 1], ["vibrato_rate", 1, 10, 0.1], ["vibrato_delay", 0, 1, 0.05], ["portamento", 0, 0.3, 0.01], ["breath", 0, 1, 0.05], ["formant_shift", -6, 6, 0.5]]) {
    const r = document.createElement("div");
    r.className = "row";
    r.innerHTML = `<span>${k}</span><input type="range" min="${lo}" max="${hi}" step="${st}" value="${t.expression[k]}">`;
    r.querySelector("input").onchange = (e) => apply({ cmd: "set_expression", track: t.id, expression: { ...t.expression, [k]: +e.target.value } });
    box.append(r);
  }
}

// ───────────────────────── canvas drawing ─────────────────────────
const cv = $("roll"), g = cv.getContext("2d");
let W = 0, H = 0;

function getPeaks(assetId) {
  if (!peaksCache.has(assetId)) peaksCache.set(assetId, eng.asset_peaks(assetId, 600));
  return peaksCache.get(assetId);
}
const pitchY = (p) => RULER_H + (HIGH - p) * KEY_H - scrollY;
const noteRect = (n, dT = 0, dP = 0, dD = 0) => ({
  x: tickToX(n.start + dT), y: pitchY(n.pitch + dP), w: Math.max(4, (n.duration + dD) * ppt - 1), h: KEY_H - 1,
});

function draw() {
  if (!P) return;
  const dpr = devicePixelRatio || 1, r = cv.getBoundingClientRect();
  W = r.width; H = r.height;
  cv.width = W * dpr; cv.height = H * dpr;
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  g.fillStyle = "#0e0e11"; g.fillRect(0, 0, W, H);
  if (view === "roll") drawRoll(); else drawArrange();
  drawRuler();
  const px = tickToX(playing ? curTick() : playhead);
  if (px >= gutter() && px <= W) {
    g.strokeStyle = "#ff5a5a"; g.lineWidth = 1.5;
    g.beginPath(); g.moveTo(px, 0); g.lineTo(px, H); g.stroke(); g.lineWidth = 1;
  }
  updatePos();
}

function gridLines(top, bottom) {
  const beat = PPQ * 4 / P.time_sig_den, bar = beat * P.time_sig_num;
  const startT = Math.floor(scrollX / beat) * beat, endT = xToTick(W);
  for (let t = startT; t <= endT; t += beat) {
    const x = tickToX(t);
    if (x < gutter()) continue;
    g.strokeStyle = t % bar === 0 ? "#3b3b48" : "#1f1f27";
    g.beginPath(); g.moveTo(x + 0.5, top); g.lineTo(x + 0.5, bottom); g.stroke();
  }
}

function drawRuler() {
  const beat = PPQ * 4 / P.time_sig_den, bar = beat * P.time_sig_num;
  g.fillStyle = "#17171c"; g.fillRect(0, 0, W, RULER_H);
  g.fillStyle = "#0e0e11"; g.fillRect(0, 0, gutter(), RULER_H);
  const lr = drag?.type === "ruler" && drag.moved ? drag.region : P.loop_region;
  if (lr) {
    g.fillStyle = "rgba(212,175,55,0.28)";
    g.fillRect(tickToX(lr.start), 0, (lr.end - lr.start) * ppt, RULER_H);
    g.fillStyle = "#d4af37"; g.fillRect(tickToX(lr.start), RULER_H - 3, (lr.end - lr.start) * ppt, 3);
  }
  g.font = "10px sans-serif"; g.textBaseline = "middle";
  const startT = Math.floor(scrollX / beat) * beat, endT = xToTick(W);
  for (let t = startT; t <= endT; t += beat) {
    const x = tickToX(t);
    if (x < gutter()) continue;
    const isBar = t % bar === 0;
    g.strokeStyle = isBar ? "#777" : "#444";
    g.beginPath(); g.moveTo(x + 0.5, isBar ? 4 : 16); g.lineTo(x + 0.5, RULER_H); g.stroke();
    if (isBar && ppt * bar > 30) { g.fillStyle = "#bbb"; g.fillText(String(t / bar + 1), x + 4, 10); }
  }
  g.strokeStyle = "#2a2a32"; g.beginPath(); g.moveTo(0, RULER_H + 0.5); g.lineTo(W, RULER_H + 0.5); g.stroke();
}

function drawRoll() {
  const bottom = H - VEL_H;
  g.save(); g.beginPath(); g.rect(0, RULER_H, W, bottom - RULER_H); g.clip();
  for (let n = LOW; n <= HIGH; n++) {
    const y = pitchY(n), black = [1, 3, 6, 8, 10].includes(n % 12);
    g.fillStyle = black ? "#121216" : "#17171d"; g.fillRect(gutter(), y, W, KEY_H);
    const isPressed = n === activePreviewKey;
    g.fillStyle = isPressed ? (black ? "#d4af37" : "#ffec99") : (black ? "#25252b" : "#d8d8dc");
    g.fillRect(0, y, gutter(), KEY_H - 1);
    if (isPressed) {
      g.fillStyle = "rgba(212, 175, 55, 0.18)";
      g.fillRect(gutter(), y, W - gutter(), KEY_H);
      g.fillStyle = black ? "#000" : "#111";
      g.font = "bold 9px sans-serif";
      g.textBaseline = "alphabetic";
      g.fillText(pitchName(n), 4, y + 10);
    } else if (n % 12 === 0) {
      g.fillStyle = "#444";
      g.font = "9px sans-serif";
      g.textBaseline = "alphabetic";
      g.fillText("C" + (n / 12 - 1), 6, y + 10);
    }
  }
  gridLines(RULER_H, bottom);
  for (const t of P.tracks) {
    if (t.id === sel || t.kind.type === "audio") continue;
    g.globalAlpha = 0.22;
    g.fillStyle = t.kind.type === "vocal" ? "#d4af37" : "#4aa3c9";
    for (const n of t.notes) { const r = noteRect(n); g.fillRect(r.x, r.y, r.w, r.h); }
  }
  g.globalAlpha = 1;
  const t = track();
  if (t && t.kind.type !== "audio") {
    const base = t.kind.type === "vocal" ? "#d4af37" : "#4aa3c9";
    const moving = drag?.type === "move";
    for (const n of t.notes) {
      const s = selNotes.has(n.id);
      let dD = drag?.type === "resize" && s ? drag.dDur : 0;
      const r = noteRect(n, 0, 0, dD);
      g.globalAlpha = moving && s ? 0.3 : 1;
      g.fillStyle = s ? "#fff3c4" : base;
      g.globalAlpha *= 0.55 + 0.45 * (n.velocity / 127);
      g.fillRect(r.x, r.y, r.w, r.h);
      g.globalAlpha = 1;
      if (s) { g.strokeStyle = "#fff"; g.strokeRect(r.x + 0.5, r.y + 0.5, r.w - 1, r.h - 1); }
      if (n.lyric) { g.fillStyle = "#000"; g.font = "10px sans-serif"; g.textBaseline = "alphabetic"; g.fillText(n.lyric, r.x + 3, r.y + 10); }
    }
    if (moving) {
      g.globalAlpha = 0.7;
      for (const n of t.notes) if (selNotes.has(n.id)) {
        const r = noteRect(n, drag.dTick, drag.dPitch);
        g.fillStyle = "#fff3c4"; g.fillRect(r.x, r.y, r.w, r.h);
      }
      g.globalAlpha = 1;
    }
    if (drag?.type === "band") {
      g.strokeStyle = "#9cf"; g.fillStyle = "rgba(120,180,255,0.15)";
      const x = Math.min(drag.x0, drag.x1), y = Math.min(drag.y0, drag.y1), w = Math.abs(drag.x1 - drag.x0), h = Math.abs(drag.y1 - drag.y0);
      g.fillRect(x, y, w, h); g.strokeRect(x + 0.5, y + 0.5, w, h);
    }
  }
  g.restore();
  // velocity lane
  g.fillStyle = "#121216"; g.fillRect(0, bottom, W, VEL_H);
  g.strokeStyle = "#2a2a32"; g.beginPath(); g.moveTo(0, bottom + 0.5); g.lineTo(W, bottom + 0.5); g.stroke();
  g.fillStyle = "#666"; g.font = "9px sans-serif"; g.textBaseline = "alphabetic"; g.fillText("Velocity", 6, bottom + 12);
  gridLines(bottom, H);
  if (t && t.kind.type !== "audio") for (const n of t.notes) {
    const x = tickToX(n.start);
    if (x < gutter() || x > W) continue;
    const v = drag?.type === "vel" && drag.vals.has(n.id) ? drag.vals.get(n.id) : n.velocity;
    const h = (v / 127) * (VEL_H - 10);
    g.fillStyle = selNotes.has(n.id) ? "#fff3c4" : "#d4af37";
    g.fillRect(x, H - 2 - h, 4, h);
  }
}

function drawArrange() {
  g.save(); g.beginPath(); g.rect(0, RULER_H, W, H - RULER_H); g.clip();
  P.tracks.forEach((t, i) => {
    const y = RULER_H + i * LANE_H - scrollY;
    if (y > H || y + LANE_H < RULER_H) return;
    g.fillStyle = i % 2 ? "#121216" : "#15151a"; g.fillRect(0, y, W, LANE_H);
    gridLines(y, y + LANE_H);
    g.strokeStyle = "#24242b"; g.beginPath(); g.moveTo(0, y + LANE_H + 0.5); g.lineTo(W, y + LANE_H + 0.5); g.stroke();
    const col = t.kind.type === "vocal" ? "#d4af37" : t.kind.type === "audio" ? "#6fcf97" : "#4aa3c9";
    // label gutter
    g.fillStyle = t.id === sel ? "#26261f" : "#18181d"; g.fillRect(0, y, gutter(), LANE_H);
    g.fillStyle = col; g.fillRect(0, y, 4, LANE_H);
    g.fillStyle = t.mute ? "#666" : "#ddd"; g.font = "11px sans-serif"; g.textBaseline = "alphabetic";
    g.fillText(t.name.slice(0, 14), 10, y + 20);
    g.fillStyle = "#777"; g.font = "9px sans-serif";
    g.fillText(t.kind.type + (t.solo ? " · SOLO" : "") + (t.mute ? " · MUTE" : ""), 10, y + 34);
    if (t.id === sel) { g.strokeStyle = "#d4af37"; g.strokeRect(0.5, y + 0.5, W - 1, LANE_H - 1); }
    // content
    if (t.kind.type === "audio") {
      const aTotal = (asset) => {
        const item = P.assets.find((x) => x.id === asset);
        return item ? item.duration_sec * P.tempo * PPQ / 60 : 1;
      };

      for (const c of t.clips) {
        let cStart = c.start;
        let cLen = clipLen(c);
        let cOff = c.offset_ticks || 0;
        let cFi = c.fade_in_ticks || 0;
        let cFo = c.fade_out_ticks || 0;

        if (drag?.clip === c.id) {
          if (drag.type === "clip") cStart = Math.max(0, c.start + drag.dTick);
          else if (drag.type === "trim_start") {
            const shift = Math.max(-c.start, Math.min(cLen - (snap <= 1 ? 30 : snap), drag.dTick));
            cStart += shift;
            cOff = Math.max(0, cOff + shift);
            cLen = Math.max(30, cLen - shift);
          } else if (drag.type === "trim_end") {
            cLen = Math.max(30, cLen + drag.dTick);
          } else if (drag.type === "fade_in") {
            cFi = Math.max(0, Math.min(cLen, (drag.orig || 0) + drag.dTick));
          } else if (drag.type === "fade_out") {
            cFo = Math.max(0, Math.min(cLen, (drag.orig || 0) + drag.dTick));
          }
        }

        const x = tickToX(cStart), w = cLen * ppt;
        if (x + w < gutter() || x > W) continue;

        // Clip container background
        g.fillStyle = c.id === selClip ? "rgba(111,207,151,0.40)" : "rgba(111,207,151,0.20)";
        g.fillRect(x, y + 4, w, LANE_H - 8);

        // Clip border
        g.strokeStyle = c.id === selClip ? "#fff" : col;
        g.strokeRect(x + 0.5, y + 4.5, w, LANE_H - 9);

        // Waveform
        const pk = getPeaks(c.asset);
        const totalAssetTicks = aTotal(c.asset);
        const startFrac = Math.max(0, cOff / totalAssetTicks);
        const durFrac = Math.max(0, cLen / totalAssetTicks);

        g.fillStyle = col;
        const mid = y + LANE_H / 2, amp = (LANE_H - 18) / 2;
        for (let px = 0; px < w; px += 1) {
          const frac = startFrac + (px / Math.max(1, w)) * durFrac;
          const idx = Math.min(pk.length - 1, Math.max(0, Math.floor(frac * pk.length)));
          const v = pk[idx] ?? 0;
          g.fillRect(x + px, mid - v * amp, 1, Math.max(1, v * amp * 2));
        }

        // Fade in slope
        if (cFi > 0) {
          const fiW = Math.min(w, cFi * ppt);
          g.fillStyle = "rgba(0,0,0,0.45)";
          g.beginPath();
          g.moveTo(x, y + 4);
          g.lineTo(x + fiW, y + 4);
          g.lineTo(x, y + LANE_H - 4);
          g.closePath();
          g.fill();
          g.strokeStyle = "#fff";
          g.lineWidth = 1.5;
          g.beginPath();
          g.moveTo(x, y + LANE_H - 4);
          g.lineTo(x + fiW, y + 4);
          g.stroke();
          g.lineWidth = 1;
        }

        // Fade out slope
        if (cFo > 0) {
          const foW = Math.min(w, cFo * ppt);
          g.fillStyle = "rgba(0,0,0,0.45)";
          g.beginPath();
          g.moveTo(x + w, y + 4);
          g.lineTo(x + w - foW, y + 4);
          g.lineTo(x + w, y + LANE_H - 4);
          g.closePath();
          g.fill();
          g.strokeStyle = "#fff";
          g.lineWidth = 1.5;
          g.beginPath();
          g.moveTo(x + w - foW, y + 4);
          g.lineTo(x + w, y + LANE_H - 4);
          g.stroke();
          g.lineWidth = 1;
        }

        // Trim handles visual indicator
        g.fillStyle = "rgba(255,255,255,0.3)";
        g.fillRect(x, y + 4, 3, LANE_H - 8);
        g.fillRect(x + w - 3, y + 4, 3, LANE_H - 8);

        // Clip label
        const assetObj = P.assets.find((a) => a.id === c.asset);
        g.fillStyle = "#fff";
        g.font = "9px sans-serif";
        g.fillText(`${assetObj?.name || "Audio"}${c.gain !== 1 ? ` (${(c.gain || 1).toFixed(1)}x)` : ""}`, x + 5, y + 14);
      }
    } else if (t.notes.length) {
      const lo = Math.min(...t.notes.map((n) => n.pitch)), hi = Math.max(...t.notes.map((n) => n.pitch), lo + 11);
      g.fillStyle = col; g.globalAlpha = 0.9;
      for (const n of t.notes) {
        const ny = y + 6 + (1 - (n.pitch - lo) / (hi - lo)) * (LANE_H - 14);
        g.fillRect(tickToX(n.start), ny, Math.max(2, n.duration * ppt - 1), 3);
      }
      g.globalAlpha = 1;
    }
    if (t.gain_auto.length) {
      g.strokeStyle = "#7ad"; g.setLineDash([4, 3]); g.beginPath();
      t.gain_auto.forEach((p, k) => {
        const px = tickToX(p.tick), py = y + LANE_H - 6 - (p.value / 2) * (LANE_H - 12);
        k ? g.lineTo(px, py) : g.moveTo(px, py);
      });
      g.stroke(); g.setLineDash([]);
    }
  });
  g.restore();
}

// ───────────────────────── transport & real-time AudioWorklet ─────────────────────────
async function initWorklet() {
  ctx ??= new AudioContext({ sampleRate: eng.sample_rate() });
  if (ctx.state === "suspended") await ctx.resume();
  if (!workletNode) {
    try {
      await ctx.audioWorklet.addModule("worklet.js");
      workletNode = new AudioWorkletNode(ctx, "kymion-worklet-processor");
      workletNode.connect(ctx.destination);
      workletNode.port.onmessage = (e) => onWorkletMessage(e.data);
    } catch (err) {
      console.warn("AudioWorklet unavailable, using fallback player:", err);
    }
  }
}

function onWorkletMessage(msg) {
  if (msg.type === "status") {
    workletBuffered = msg.buffered;
    if (msg.playing) {
      const sr = eng.sample_rate();
      playhead = (msg.totalSamples / sr) / spt();
      updatePos();

      // Master meter display
      const pL = Math.min(100, Math.round(Math.pow(msg.peakL, 0.6) * 100));
      const pR = Math.min(100, Math.round(Math.pow(msg.peakR, 0.6) * 100));
      const ml = $("meter-l"), mr = $("meter-r");
      if (ml) ml.style.width = pL + "%";
      if (mr) mr.style.width = pR + "%";

      // Track meters update
      const now = performance.now();
      if (now - lastMeterUpdate > 45) {
        lastMeterUpdate = now;
        try {
          const metersJson = eng.meter_levels(msg.totalSamples, 2048);
          const m = JSON.parse(metersJson);
          if (m.tracks) {
            for (const [tid, tMeter] of Object.entries(m.tracks)) {
              const el = document.querySelector(`.trk-meter-fill[data-meter-tid="${tid}"]`);
              if (el) {
                const val = Math.min(100, Math.round(Math.pow(tMeter.peak, 0.6) * 100));
                el.style.height = val + "%";
              }
            }
          }
        } catch {}
      }
    }
  }
}

function feedWorklet() {
  if (!playing || !workletNode) return;
  const CHUNK = 8192;
  while (workletBuffered < 16384) {
    const slice = eng.render_slice(streamPos, CHUNK, metro);
    workletNode.port.postMessage({ type: "push", data: slice }, [slice.buffer]);
    streamPos += CHUNK;
    workletBuffered += CHUNK;
  }
}

function curTick() {
  if (!playing) return playhead;
  if (workletNode) return playhead;
  let t = startTick + (ctx.currentTime - t0) / spt();
  const lr = P.loop_region;
  if (lr && t >= lr.end) t = lr.start + ((t - lr.start) % (lr.end - lr.start));
  return t;
}

function updatePos() {
  if (!P) return;
  const t = Math.max(0, curTick());
  const beat = PPQ * 4 / P.time_sig_den, bar = beat * P.time_sig_num;
  const b = Math.floor(t / bar) + 1, bt = Math.floor((t % bar) / beat) + 1, tk = Math.floor(t % beat);
  $("pos").textContent = `${String(b).padStart(3, "0")}:${bt}:${String(tk).padStart(3, "0")}  ${(t * spt()).toFixed(2)}s`;
}

function clearMeters() {
  const ml = $("meter-l"), mr = $("meter-r");
  if (ml) ml.style.width = "0%";
  if (mr) mr.style.width = "0%";
  document.querySelectorAll(".trk-meter-fill").forEach((el) => (el.style.height = "0%"));
}

function stopSrc() {
  if (srcNode) { srcNode.onended = null; try { srcNode.stop(); } catch {} srcNode = null; }
}

async function play(from = playhead) {
  stopSrc();
  await initWorklet();
  const lr = P.loop_region;
  if (lr && from >= lr.end) from = lr.start;
  playhead = from;

  if (workletNode) {
    const sr = eng.sample_rate();
    streamPos = Math.round(from * spt() * sr);
    workletNode.port.postMessage({ type: "clear" });
    workletBuffered = 0;
    workletNode.port.postMessage({
      type: "set_playing",
      playing: true,
      totalSamples: streamPos,
    });
    feedWorklet();
    playing = true;
    $("play").textContent = "Pause";
    requestAnimationFrame(frame);
  } else {
    ctx ??= new AudioContext({ sampleRate: eng.sample_rate() });
    await ctx.resume();
    const data = eng.render_opts(metro), n = data.length / 2, sr = eng.sample_rate();
    const buf = ctx.createBuffer(2, Math.max(n, 1), sr);
    const L = buf.getChannelData(0), R = buf.getChannelData(1);
    for (let i = 0; i < n; i++) { L[i] = data[2 * i]; R[i] = data[2 * i + 1]; }
    const s = ctx.createBufferSource();
    s.buffer = buf; s.connect(ctx.destination);
    if (lr) { s.loop = true; s.loopStart = lr.start * spt(); s.loopEnd = lr.end * spt(); }
    s.start(0, Math.max(0, from * spt()));
    s.onended = () => { if (srcNode === s) { srcNode = null; playing = false; playhead = Math.min(playhead, from); $("play").textContent = "Play"; clearMeters(); draw(); } };
    srcNode = s; t0 = ctx.currentTime; startTick = from; playing = true;
    $("play").textContent = "Pause";
    requestAnimationFrame(frame);
  }
}

function frame() {
  if (!playing) return;
  feedWorklet();
  const t = curTick();
  const x = tickToX(t);
  if (x > W - 40) scrollX = t - 40 / ppt;
  if (x < gutter()) scrollX = Math.max(0, t);
  draw();
  requestAnimationFrame(frame);
}

function pause() {
  if (workletNode) {
    workletNode.port.postMessage({ type: "set_playing", playing: false });
    workletNode.port.postMessage({ type: "clear" });
  }
  stopSrc();
  playing = false;
  $("play").textContent = "Play";
  clearMeters();
  draw();
}

function stopAll() {
  if (workletNode) {
    workletNode.port.postMessage({ type: "set_playing", playing: false });
    workletNode.port.postMessage({ type: "clear" });
  }
  stopSrc();
  playing = false;
  $("play").textContent = "Play";
  playhead = P.loop_region ? P.loop_region.start : 0;
  scrollX = Math.max(0, Math.min(scrollX, playhead));
  clearMeters();
  draw();
}

function seek(t) {
  playhead = Math.max(0, t);
  if (playing) play(playhead); else draw();
}

// ───────────────────────── pointer interaction ─────────────────────────
const local = (e) => { const r = cv.getBoundingClientRect(); return [e.clientX - r.left, e.clientY - r.top]; };

function noteAt(x, y) {
  const t = track(); if (!t || t.kind.type === "audio") return null;
  for (let i = t.notes.length - 1; i >= 0; i--) {
    const r = noteRect(t.notes[i]);
    if (x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h) return t.notes[i];
  }
  return null;
}

cv.addEventListener("pointerdown", (e) => {
  const [x, y] = local(e);
  cv.setPointerCapture(e.pointerId);
  if (y < RULER_H && x > gutter()) {
    const t = snapRound(xToTick(x));
    drag = { type: "ruler", x0: x, t0: t, moved: false, region: null };
    seek(t);
    return;
  }
  if (view === "roll") rollDown(x, y, e); else arrangeDown(x, y, e);
});

function rollDown(x, y, e) {
  const t = track();
  if (!t || t.kind.type === "audio") return;
  if (x < gutter()) {
    if (y >= RULER_H && y <= H - VEL_H) {
      const pitch = yToPitch(y);
      if (pitch >= LOW && pitch <= HIGH) {
        activePreviewKey = pitch;
        drag = { type: "piano_key", pitch };
        playNotePreview(t.id, pitch, 0.4, 100);
        draw();
      }
    }
    return;
  }
  if (y > H - VEL_H) {
    // velocity lane: pick nearest note stem
    const hit = t.notes.find((n) => Math.abs(tickToX(n.start) + 2 - x) <= 5);
    if (!hit) return;
    if (!selNotes.has(hit.id)) { if (!e.shiftKey) selNotes.clear(); selNotes.add(hit.id); }
    drag = { type: "vel", vals: new Map(), targets: [...selNotes] };
    velMove(y);
    return;
  }
  if (y < RULER_H) return;
  const hit = noteAt(x, y);
  if (hit) {
    if (e.shiftKey && selNotes.has(hit.id)) { selNotes.delete(hit.id); draw(); return; }
    if (!selNotes.has(hit.id)) { if (!e.shiftKey) selNotes.clear(); selNotes.add(hit.id); }
    const edge = x > tickToX(hit.start + hit.duration) - 6;
    drag = { type: edge ? "resize" : "move", x0: x, y0: y, dTick: 0, dPitch: 0, dDur: 0, lastAuditionPitch: hit.pitch };
    playNotePreview(t.id, hit.pitch, 0.25, hit.velocity);
    draw();
    return;
  }
  if (tool === "draw") {
    const start = snapFloor(xToTick(x)), pitch = yToPitch(y);
    const out = apply({ cmd: "add_notes", track: t.id, notes: [{ start, duration: snap <= 1 ? 240 : snap, pitch, velocity: 100, lyric: t.kind.type === "vocal" ? "la" : null }] });
    if (out.ok) {
      selNotes = new Set(out.note_ids);
      drag = { type: "resize", x0: x, y0: y, dTick: 0, dPitch: 0, dDur: 0 };
      playNotePreview(t.id, pitch, 0.35, 100);
    }
  } else {
    if (!e.shiftKey) selNotes.clear();
    drag = { type: "band", x0: x, y0: y, x1: x, y1: y, base: new Set(selNotes) };
    draw();
  }
}

function velMove(y) {
  const v = Math.max(1, Math.min(127, Math.round(((H - 2 - y) / (VEL_H - 10)) * 127)));
  drag.vals = new Map(drag.targets.map((id) => [id, v]));
  draw();
}

function arrangeDown(x, y, e) {
  if (y < RULER_H) return;
  const idx = Math.floor((y - RULER_H + scrollY) / LANE_H), t = P.tracks[idx];
  if (!t) return;
  sel = t.id; selNotes.clear();
  if (x >= gutter() && t.kind.type === "audio") {
    const tick = xToTick(x);
    const c = t.clips.find((c) => tick >= c.start && tick <= c.start + clipLen(c));
    if (c) {
      selClip = c.id;
      const cX = tickToX(c.start);
      const cW = clipLen(c) * ppt;
      const cY = RULER_H + idx * LANE_H - scrollY;
      const fiW = (c.fade_in_ticks || 0) * ppt;
      const foW = (c.fade_out_ticks || 0) * ppt;

      if (y <= cY + 18 && Math.abs(x - (cX + fiW)) <= 10) {
        drag = { type: "fade_in", clip: c.id, track: t.id, x0: x, orig: c.fade_in_ticks || 0, dTick: 0 };
      } else if (y <= cY + 18 && Math.abs(x - (cX + cW - foW)) <= 10) {
        drag = { type: "fade_out", clip: c.id, track: t.id, x0: x, orig: c.fade_out_ticks || 0, dTick: 0 };
      } else if (x <= cX + 8) {
        drag = { type: "trim_start", clip: c.id, track: t.id, x0: x, start0: c.start, off0: c.offset_ticks || 0, dur0: clipLen(c), dTick: 0 };
      } else if (x >= cX + cW - 8) {
        drag = { type: "trim_end", clip: c.id, track: t.id, x0: x, dur0: clipLen(c), dTick: 0 };
      } else {
        drag = { type: "clip", clip: c.id, track: t.id, x0: x, dTick: 0, start: c.start };
      }
    } else {
      selClip = null;
    }
  } else {
    selClip = null;
  }
  refresh();
}

cv.addEventListener("pointermove", (e) => {
  const [x, y] = local(e);
  if (!drag) {
    if (view === "roll") {
      if (x < gutter() && y >= RULER_H && y <= H - VEL_H) {
        cv.style.cursor = "pointer";
      } else if (y > H - VEL_H) {
        cv.style.cursor = "ns-resize";
      } else {
        cv.style.cursor = tool === "draw" ? "crosshair" : "default";
      }
      return;
    }
    if (view === "arrange") {
      const idx = Math.floor((y - RULER_H + scrollY) / LANE_H);
      const t = P?.tracks?.[idx];
      if (t && t.kind.type === "audio" && x >= gutter() && y >= RULER_H) {
        const tick = xToTick(x);
        const c = t.clips.find((c) => tick >= c.start && tick <= c.start + clipLen(c));
        if (c) {
          const cX = tickToX(c.start);
          const cW = clipLen(c) * ppt;
          const cY = RULER_H + idx * LANE_H - scrollY;
          const fiW = (c.fade_in_ticks || 0) * ppt;
          const foW = (c.fade_out_ticks || 0) * ppt;
          if (y <= cY + 18 && (Math.abs(x - (cX + fiW)) <= 10 || Math.abs(x - (cX + cW - foW)) <= 10)) {
            cv.style.cursor = "crosshair";
          } else if (x <= cX + 8 || x >= cX + cW - 8) {
            cv.style.cursor = "ew-resize";
          } else {
            cv.style.cursor = "grab";
          }
          return;
        }
      }
      cv.style.cursor = "default";
    }
    return;
  }

  switch (drag.type) {
    case "piano_key": {
      const pitch = yToPitch(y);
      if (pitch >= LOW && pitch <= HIGH && pitch !== drag.pitch) {
        drag.pitch = pitch;
        activePreviewKey = pitch;
        const t = track();
        if (t) playNotePreview(t.id, pitch, 0.35, 100);
        draw();
      }
      return;
    }
    case "ruler": {
      if (Math.abs(x - drag.x0) > 4) {
        drag.moved = true;
        const a = drag.t0, b = snapRound(xToTick(x));
        drag.region = { start: Math.min(a, b), end: Math.max(a, b) };
      }
      break;
    }
    case "move": {
      drag.dTick = snapDelta((x - drag.x0) / ppt);
      const newDP = -Math.round((y - drag.y0) / KEY_H);
      if (newDP !== drag.dPitch) {
        drag.dPitch = newDP;
        const t = track();
        if (t) {
          const first = t.notes.find((n) => selNotes.has(n.id));
          if (first) {
            const p = Math.max(LOW, Math.min(HIGH, first.pitch + newDP));
            if (p !== drag.lastAuditionPitch) {
              drag.lastAuditionPitch = p;
              activePreviewKey = p;
              playNotePreview(t.id, p, 0.2, first.velocity);
            }
          }
        }
      }
      break;
    }
    case "resize": drag.dDur = snapDelta((x - drag.x0) / ppt); break;
    case "band": {
      drag.x1 = x; drag.y1 = y;
      const t = track();
      const [xa, xb] = [Math.min(drag.x0, x), Math.max(drag.x0, x)], [ya, yb] = [Math.min(drag.y0, y), Math.max(drag.y0, y)];
      selNotes = new Set(drag.base);
      for (const n of t.notes) { const r = noteRect(n); if (r.x < xb && r.x + r.w > xa && r.y < yb && r.y + r.h > ya) selNotes.add(n.id); }
      break;
    }
    case "vel": velMove(y); return;
    case "clip": drag.dTick = snapDelta((x - drag.x0) / ppt); if (drag.start + drag.dTick < 0) drag.dTick = -drag.start; break;
    case "trim_start": drag.dTick = snapDelta((x - drag.x0) / ppt); break;
    case "trim_end": drag.dTick = snapDelta((x - drag.x0) / ppt); break;
    case "fade_in": drag.dTick = snapDelta((x - drag.x0) / ppt); break;
    case "fade_out": drag.dTick = snapDelta((drag.x0 - x) / ppt); break;
  }
  draw();
});

cv.addEventListener("pointerup", () => {
  if (activePreviewKey !== null) {
    activePreviewKey = null;
    draw();
  }
  if (!drag) return;
  const d = drag; drag = null;
  if (d.type === "piano_key") return;
  const t = track();
  switch (d.type) {
    case "ruler":
      if (d.moved && d.region && d.region.end > d.region.start) apply({ cmd: "set_loop", start: d.region.start, end: d.region.end });
      else draw();
      return;
    case "move":
      if (d.dTick || d.dPitch) apply({ cmd: "move_notes", track: t.id, notes: [...selNotes], delta_tick: d.dTick, delta_pitch: d.dPitch });
      else draw();
      return;
    case "resize":
      if (d.dDur) {
        for (const n of t.notes.filter((n) => selNotes.has(n.id)))
          eng.apply(JSON.stringify({ cmd: "move_note", track: t.id, note: n.id, duration: Math.max(snap <= 1 ? 30 : snap, n.duration + d.dDur) }));
        refresh();
      } else draw();
      return;
    case "vel": {
      const v = [...d.vals.values()][0];
      if (v) apply({ cmd: "set_velocity", track: t.id, notes: d.targets, velocity: v }); else draw();
      return;
    }
    case "clip":
      if (d.dTick) apply({ cmd: "move_clip", track: d.track, clip: d.clip, start: d.start + d.dTick }); else draw();
      return;
    case "trim_start": {
      const shift = Math.max(-d.start0, Math.min(d.dur0 - (snap <= 1 ? 30 : snap), d.dTick));
      if (shift !== 0) {
        const newStart = d.start0 + shift;
        const newOffset = Math.max(0, d.off0 + shift);
        const newDur = Math.max(30, d.dur0 - shift);
        apply({ cmd: "trim_clip", track: d.track, clip: d.clip, start: newStart, offset_ticks: newOffset, duration_ticks: newDur });
      } else draw();
      return;
    }
    case "trim_end": {
      const newDur = Math.max(snap <= 1 ? 30 : snap, d.dur0 + d.dTick);
      if (newDur !== d.dur0) {
        apply({ cmd: "trim_clip", track: d.track, clip: d.clip, duration_ticks: newDur });
      } else draw();
      return;
    }
    case "fade_in": {
      const newFi = Math.max(0, d.orig + d.dTick);
      apply({ cmd: "set_clip_fade", track: d.track, clip: d.clip, fade_in_ticks: newFi });
      return;
    }
    case "fade_out": {
      const newFo = Math.max(0, d.orig + d.dTick);
      apply({ cmd: "set_clip_fade", track: d.track, clip: d.clip, fade_out_ticks: newFo });
      return;
    }
    default: draw();
  }
});

cv.addEventListener("dblclick", (e) => {
  const [x, y] = local(e);
  if (view === "arrange") {
    const idx = Math.floor((y - RULER_H + scrollY) / LANE_H);
    const t = P.tracks[idx];
    if (t) {
      if (t.kind.type === "audio") {
        const tick = xToTick(x);
        const c = t.clips.find((c) => tick >= c.start && tick <= c.start + clipLen(c));
        if (c) {
          const g = prompt("Clip Gain multiplier (0.0 - 2.0)", (c.gain || 1.0).toFixed(2));
          if (g != null && !isNaN(+g)) apply({ cmd: "set_clip_gain", track: t.id, clip: c.id, gain: +g });
          return;
        }
      } else {
        sel = t.id; view = "roll"; scrollY = Math.max(0, (HIGH - 72) * KEY_H - 90); refresh();
        return;
      }
    }
    return;
  }
  const n = noteAt(x, y), t = track();
  if (n && t) {
    const l = prompt("Lyric syllable", n.lyric ?? "la");
    if (l != null) apply({ cmd: "set_lyrics", track: t.id, lyrics: t.notes.map((m) => (m.id === n.id ? l : m.lyric ?? "la")) });
  }
});
cv.addEventListener("contextmenu", (e) => {
  e.preventDefault();
  const [x, y] = local(e), n = noteAt(x, y), t = track();
  if (view === "roll" && n && t) apply({ cmd: "remove_notes", track: t.id, notes: [n.id] });
  else if (y < RULER_H && P.loop_region) apply({ cmd: "set_loop" });
});
cv.addEventListener("wheel", (e) => {
  e.preventDefault();
  if (e.ctrlKey || e.metaKey) {
    const [x] = local(e), anchor = xToTick(x);
    ppt = Math.max(0.02, Math.min(1.2, ppt * Math.exp(-e.deltaY * 0.0025)));
    scrollX = Math.max(0, anchor - (x - gutter()) / ppt);
  } else if (e.shiftKey || Math.abs(e.deltaX) > Math.abs(e.deltaY)) {
    scrollX = Math.max(0, scrollX + (e.deltaX || e.deltaY) / ppt);
  } else {
    const max = view === "roll" ? (HIGH - LOW + 1) * KEY_H - (H - RULER_H - VEL_H) : Math.max(0, P.tracks.length * LANE_H - (H - RULER_H));
    scrollY = Math.max(0, Math.min(Math.max(0, max), scrollY + e.deltaY));
  }
  draw();
}, { passive: false });
addEventListener("resize", () => draw());

// ───────────────────────── keyboard ─────────────────────────
function selected() { return track()?.notes.filter((n) => selNotes.has(n.id)) ?? []; }
function paste(at) {
  const t = track(); if (!t || !clipboard.length) return;
  const out = apply({ cmd: "add_notes", track: t.id, notes: clipboard.map((n) => ({ ...n, start: n.start + at })) });
  if (out.ok) { selNotes = new Set(out.note_ids); draw(); }
}
addEventListener("keydown", (e) => {
  if (/INPUT|TEXTAREA|SELECT/.test(e.target.tagName)) return;
  const mod = e.ctrlKey || e.metaKey, t = track(), key = e.key.toLowerCase();
  if (key === " ") { e.preventDefault(); playing ? pause() : play(); }
  else if (key === "enter") stopAll();
  else if (mod && key === "z") { e.preventDefault(); try { e.shiftKey ? eng.redo() : eng.undo(); refresh(); } catch (err) { log(String(err)); } }
  else if (mod && key === "y") { try { eng.redo(); refresh(); } catch (err) { log(String(err)); } }
  else if (mod && key === "a" && t && view === "roll") { e.preventDefault(); selNotes = new Set(t.notes.map((n) => n.id)); draw(); }
  else if (mod && key === "c") { const s = selected(); if (s.length) { const m = Math.min(...s.map((n) => n.start)); clipboard = s.map((n) => ({ start: n.start - m, duration: n.duration, pitch: n.pitch, velocity: n.velocity, lyric: n.lyric ?? null })); log(`Copied ${s.length} note(s)`); } }
  else if (mod && key === "v") { e.preventDefault(); paste(snapRound(curTick())); }
  else if (mod && key === "d") { e.preventDefault(); const s = selected(); if (s.length) { const m = Math.min(...s.map((n) => n.start)), end = Math.max(...s.map((n) => n.start + n.duration)); clipboard = s.map((n) => ({ start: n.start - m, duration: n.duration, pitch: n.pitch, velocity: n.velocity, lyric: n.lyric ?? null })); paste(end); } }
  else if (key === "delete" || key === "backspace") {
    e.preventDefault();
    if (selNotes.size && t) apply({ cmd: "remove_notes", track: t.id, notes: [...selNotes] });
    else if (selClip !== null && t) { apply({ cmd: "remove_clip", track: t.id, clip: selClip }); selClip = null; }
  }
  else if (key === "s" && !mod) {
    if (view === "arrange" && selClip !== null && t && t.kind.type === "audio") {
      const at = Math.round(snapRound(curTick()));
      const c = t.clips.find((x) => x.id === selClip);
      if (c && at > c.start && at < c.start + clipLen(c)) {
        apply({ cmd: "split_clip", track: t.id, clip: c.id, at });
      }
    } else {
      for (const n of selected()) {
        const at = snapRound(curTick());
        if (at > n.start && at < n.start + n.duration) {
          eng.apply(JSON.stringify({ cmd: "split_note", track: t.id, note: n.id, at }));
        }
      }
      refresh();
    }
  }
  else if (key === "l") $("loop").click();
  else if (key === "m" && !mod) $("metro").click();
  else if (key === "d") $("tool-draw").click();
  else if (key === "v") $("tool-select").click();
  else if (key === "tab") { e.preventDefault(); view = view === "roll" ? "arrange" : "roll"; refresh(); }
  else if (key.startsWith("arrow") && selNotes.size && t) {
    e.preventDefault();
    const step = snap <= 1 ? 120 : snap;
    const dt = key === "arrowright" ? step : key === "arrowleft" ? -step : 0;
    const dp = key === "arrowup" ? (e.shiftKey ? 12 : 1) : key === "arrowdown" ? (e.shiftKey ? -12 : -1) : 0;
    apply({ cmd: "move_notes", track: t.id, notes: [...selNotes], delta_tick: dt, delta_pitch: dp });
    if (dp !== 0) {
      const first = t.notes.find((n) => selNotes.has(n.id));
      if (first) {
        activePreviewKey = first.pitch;
        playNotePreview(t.id, first.pitch, 0.25, first.velocity);
        setTimeout(() => {
          if (activePreviewKey === first.pitch) {
            activePreviewKey = null;
            draw();
          }
        }, 220);
      }
    }
  }
});

// ───────────────────────── toolbar wiring ─────────────────────────
$("play").onclick = () => (playing ? pause() : play());
$("stop").onclick = stopAll;
$("loop").onclick = () => {
  if (P.loop_region) return apply({ cmd: "set_loop" });
  const bar = PPQ * 4 / P.time_sig_den * P.time_sig_num;
  const len = Math.max(0, ...P.tracks.flatMap((t) => [...t.notes.map((n) => n.start + n.duration), ...t.clips.map((c) => c.start + clipLen(c))]));
  const end = Math.max(bar * 2, Math.ceil(Math.max(len, 1) / bar) * bar);
  apply({ cmd: "set_loop", start: 0, end });
};
$("metro").onclick = () => { metro = !metro; refresh(); };
$("undo").onclick = () => { try { eng.undo(); refresh(); } catch (e) { log(String(e)); } };
$("redo").onclick = () => { try { eng.redo(); refresh(); } catch (e) { log(String(e)); } };
$("tempo").onchange = (e) => apply({ cmd: "set_tempo", bpm: +e.target.value });
$("ts-num").onchange = () => apply({ cmd: "set_time_signature", numerator: +$("ts-num").value, denominator: +$("ts-den").value });
$("ts-den").onchange = () => $("ts-num").onchange();
$("snap").onchange = (e) => { snap = +e.target.value; };
$("view-arrange").onclick = () => { view = "arrange"; refresh(); };
$("view-roll").onclick = () => { view = "roll"; refresh(); };
$("tool-draw").onclick = () => { tool = "draw"; refresh(); };
$("tool-select").onclick = () => { tool = "select"; refresh(); };
$("zoom-in").onclick = () => { ppt = Math.min(1.2, ppt * 1.4); draw(); };
$("zoom-out").onclick = () => { ppt = Math.max(0.02, ppt / 1.4); draw(); };
$("quantize").onclick = () => { if (sel != null) apply({ cmd: "quantize", track: sel, grid: snap <= 1 ? 120 : snap }); };

$("add-inst").onclick = () => apply({ cmd: "add_track", name: "Instrument", kind: { type: "instrument", waveform: "pluck" } });
$("add-vocal").onclick = () => apply({ cmd: "add_track", name: "Vocal", kind: { type: "vocal", voice: "default" } });
$("add-audio").onclick = () => apply({ cmd: "add_track", name: "Audio", kind: { type: "audio" } });

const fxBtn = (id, track_, kind) => ($(id).onclick = () => { if (track_ === "sel" ? sel != null : true) apply({ cmd: "add_fx", track: track_ === "sel" ? sel : null, kind }); });
fxBtn("add-fx-eq", "sel", { type: "eq", low_gain: 2, mid_gain: -1, mid_freq: 1000, mid_q: 1, high_gain: 2 });
fxBtn("add-fx-delay", "sel", { type: "delay", time_ms: 250, feedback: 0.45, mix: 0.35, ping_pong: true });
fxBtn("add-fx-reverb", "sel", { type: "reverb", room_size: 0.75, damping: 0.25, width: 1.0, mix: 0.35 });
fxBtn("add-master-fx-eq", null, { type: "eq", low_gain: 1.5, mid_gain: 0, mid_freq: 1200, mid_q: 1, high_gain: 2.0 });
fxBtn("add-master-fx-comp", null, { type: "compressor", threshold_db: -12, ratio: 4, attack_ms: 15, release_ms: 100, makeup_db: 2 });

// automation
$("auto-param").onchange = () => {
  const pan = $("auto-param").value === "pan", v = $("auto-val");
  v.min = pan ? -1 : 0; v.max = pan ? 1 : 2; v.value = pan ? 0 : 1; $("auto-val-txt").textContent = (+v.value).toFixed(2);
};
$("auto-val").oninput = () => ($("auto-val-txt").textContent = (+$("auto-val").value).toFixed(2));
$("auto-add").onclick = () => { if (sel != null) apply({ cmd: "add_auto_point", track: sel, param: $("auto-param").value, tick: Math.round(snapRound(curTick())), value: +$("auto-val").value }); };
$("auto-clear").onclick = () => { if (sel != null) apply({ cmd: "clear_auto", track: sel, param: $("auto-param").value }); };

$("run").onclick = () => { try { const v = JSON.parse($("cmd").value); (Array.isArray(v) ? v : [v]).forEach(apply); } catch (e) { log("ERROR: " + e); } };

// ───────────────────────── files ─────────────────────────
const download = (name, data, type) => {
  const a = document.createElement("a");
  a.href = URL.createObjectURL(new Blob([data], { type }));
  a.download = name; a.click();
};
$("wav").onclick = () => download("kymion.wav", eng.wav(), "audio/wav");
$("save").onclick = () => download("project.kymion.json", eng.project_json(), "application/json");
$("load").onclick = () => $("file").click();
$("file").onchange = async (e) => { try { eng = KymionEngine.load(await e.target.files[0].text()); peaksCache.clear(); refresh(); } catch (err) { log("ERROR: " + err); } };
$("export-midi").onclick = () => { try { download("project.mid", eng.export_midi(), "audio/midi"); log("Exported MIDI."); } catch (e) { log("MIDI Export Error: " + e); } };
$("import-midi").onclick = () => $("midi-file").click();
async function loadMidi(file) {
  try { eng = KymionEngine.import_midi(new Uint8Array(await file.arrayBuffer())); peaksCache.clear(); sel = null; refresh(); log(`Imported MIDI "${file.name}".`); }
  catch (e) { log("MIDI Import Error: " + e); }
}
$("midi-file").onchange = (e) => e.target.files[0] && loadMidi(e.target.files[0]);

async function importSoundfontFile(file) {
  try {
    const bytes = new Uint8Array(await file.arrayBuffer());
    const name = file.name.replace(/\.[^.]+$/, "");
    const assetId = eng.import_soundfont(name, bytes);
    let presets = [];
    try {
      presets = JSON.parse(eng.soundfont_presets(assetId));
    } catch (_) {}
    log(`Imported SoundFont "${file.name}" with ${presets.length} presets.`);

    // If an instrument track is currently selected, assign this soundfont to it!
    const curTrack = track();
    if (curTrack && curTrack.kind.type === "instrument") {
      const defaultPreset = presets[0] || { bank: 0, preset: 0 };
      apply({
        cmd: "set_track_soundfont",
        track: curTrack.id,
        soundfont: { asset: assetId, bank: defaultPreset.bank, preset: defaultPreset.preset }
      });
      log(`Assigned "${name}" (${defaultPreset.name || "Preset 0"}) to track "${curTrack.name}".`);
    } else {
      // Or create a new track for it
      const trId = apply({
        cmd: "add_track",
        name,
        kind: {
          type: "instrument",
          waveform: "pluck",
          soundfont: { asset: assetId, bank: 0, preset: 0 }
        }
      }).track_id;
      sel = trId;
    }
    refresh();
  } catch (err) {
    log("SoundFont Import Error: " + err);
  }
}
$("import-soundfont").onclick = () => $("sf2-file").click();
$("sf2-file").onchange = (e) => e.target.files[0] && importSoundfontFile(e.target.files[0]);

async function importAudioFile(file) {
  try {
    ctx ??= new AudioContext({ sampleRate: eng.sample_rate() });
    const dec = await ctx.decodeAudioData(await file.arrayBuffer());
    const mono = new Float32Array(dec.length);
    for (let c = 0; c < dec.numberOfChannels; c++) { const d = dec.getChannelData(c); for (let i = 0; i < dec.length; i++) mono[i] += d[i] / dec.numberOfChannels; }
    const name = file.name.replace(/\.[^.]+$/, "");
    const asset = eng.import_audio(name, dec.sampleRate, mono);
    const tr = apply({ cmd: "add_track", name, kind: { type: "audio" } }).track_id;
    apply({ cmd: "add_clip", track: tr, asset, start: snapRound(curTick()) });
    sel = tr; refresh(); log(`Imported audio "${file.name}" (${dec.duration.toFixed(2)}s).`);
  } catch (e) { log("Audio Import Error: " + e); }
}
$("import-audio").onclick = () => $("audio-file").click();
$("audio-file").onchange = (e) => e.target.files[0] && importAudioFile(e.target.files[0]);

addEventListener("dragover", (e) => e.preventDefault());
addEventListener("drop", async (e) => {
  e.preventDefault();
  const f = e.dataTransfer?.files[0]; if (!f) return;
  const n = f.name.toLowerCase();
  if (/\.midi?$/.test(n)) loadMidi(f);
  else if (n.endsWith(".json")) { eng = KymionEngine.load(await f.text()); peaksCache.clear(); refresh(); log(`Loaded project "${f.name}".`); }
  else if (n.endsWith(".sf2") || n.endsWith(".soundfont")) importSoundfontFile(f);
  else importAudioFile(f);
});

// ───────────────────────── mix diagnostics ─────────────────────────
function runDiagnostics() {
  if (!eng) return;
  try {
    const res = JSON.parse(eng.diagnose_json());
    renderDiagnostics(res);
    log("Mix Diagnostics: " + res.summary);
  } catch (e) {
    log("Diagnostics error: " + e);
  }
}

function renderDiagnostics(res) {
  if (!res || !res.lufs) return;
  const lufs = res.lufs;
  const lufsEl = $("lufs-val");
  const tagEl = $("lufs-tag");
  const tpEl = $("tp-val");
  const lraEl = $("lra-val");
  const clashListEl = $("clash-list");

  if (lufsEl) lufsEl.textContent = `${lufs.integrated.toFixed(1)} LUFS`;
  if (tpEl) tpEl.textContent = `${lufs.true_peak_db.toFixed(1)} dBTP`;
  if (lraEl) lraEl.textContent = `${lufs.loudness_range.toFixed(1)} LU`;

  if (tagEl) {
    if (lufs.integrated >= -16 && lufs.integrated <= -12) {
      tagEl.textContent = "Optimal (Streaming)";
      tagEl.style.background = "#1b3a2a";
      tagEl.style.color = "#6fcf97";
    } else if (lufs.integrated > -12) {
      tagEl.textContent = "Loud (May Attenuate)";
      tagEl.style.background = "#3a2a1b";
      tagEl.style.color = "#f2c94c";
    } else {
      tagEl.textContent = "Quiet / Dynamic";
      tagEl.style.background = "#252532";
      tagEl.style.color = "#aaa";
    }
  }

  if (clashListEl) {
    if (!res.clashes || res.clashes.length === 0) {
      clashListEl.innerHTML = `<div style="font-size:10px;color:#6fcf97;padding:4px 0">Clean mix: No frequency masking clashes detected.</div>`;
    } else {
      let html = `<div style="font-size:10px;color:#eb5757;margin-bottom:4px">${res.clashes.length} Frequency Clash(es) Found:</div>`;
      for (const c of res.clashes) {
        const sevClass = c.severity === "high" ? "" : (c.severity === "medium" ? "med" : "low");
        html += `<div class="clash-card ${sevClass}">
          <div class="clash-header">
            <span>[${c.severity.toUpperCase()}] ${esc(c.track_a_name)} vs ${esc(c.track_b_name)}</span>
            <span>${c.band.toUpperCase()}</span>
          </div>
          <div class="clash-desc">${esc(c.suggestion)}</div>
        </div>`;
      }
      clashListEl.innerHTML = html;
    }
  }
}

const btnDiag = $("btn-diagnose");
if (btnDiag) {
  btnDiag.onclick = runDiagnostics;
}

// ───────────────────────── demo project ─────────────────────────
apply({
  cmd: "add_track",
  name: "Moog Synth",
  kind: {
    type: "instrument",
    waveform: "saw",
    synth: {
      filter: "moog",
      cutoff_hz: 1200,
      resonance: 0.55,
      filter_env_amount: 0.5,
      attack: 0.005,
      decay: 0.25,
      sustain: 0.4,
      release: 0.25,
    },
  },
});
apply({ cmd: "add_track", name: "Lead Vocal", kind: { type: "vocal", voice: "default" } });
apply({ cmd: "add_notes", track: 1, notes: [48, 52, 55, 60, 55, 52, 55, 64].map((p, i) => ({ start: i * 480, duration: 480, pitch: p, velocity: 90 })) });
apply({ cmd: "add_notes", track: 2, notes: [[64, "안"], [67, "녕"], [69, "키"], [72, "미"], [71, "온"]].map(([p, l], i) => ({ start: i * 720, duration: 720, pitch: p, velocity: 100, lyric: l })) });
apply({ cmd: "add_fx", track: 1, kind: { type: "reverb", room_size: 0.65, damping: 0.3, width: 1.0, mix: 0.25 } });
apply({ cmd: "add_fx", track: 2, kind: { type: "delay", time_ms: 220, feedback: 0.4, mix: 0.25, ping_pong: true } });
setTimeout(runDiagnostics, 100);

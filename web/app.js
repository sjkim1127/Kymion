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
    log(out); refresh(); return JSON.parse(out);
  } catch (e) { log("ERROR: " + e); return { ok: false, error: String(e) }; }
}
const log = (s) => { $("log").textContent = s + "\n" + $("log").textContent.slice(0, 600); };
window.kymion = { apply, project: proj, schema: () => JSON.parse(KymionEngine.schema()), analysis: () => JSON.parse(eng.analysis_json()) };

function refresh() {
  const p = proj();
  if (sel === null || !p.tracks.find((t) => t.id === sel)) sel = p.tracks[0]?.id ?? null;
  $("tempo").value = p.tempo;
  $("tracks").innerHTML = "";
  for (const t of p.tracks) {
    const d = document.createElement("div");
    d.className = "trk" + (t.id === sel ? " sel" : "");
    d.innerHTML = `<span>${t.kind.type === "vocal" ? "🎤" : "🎹"} ${t.name}</span><span class="spacer" style="flex:1"></span>
      <input type="range" min="0" max="1.5" step="0.05" value="${t.gain}" title="gain">
      <input type="checkbox" ${t.mute ? "checked" : ""} title="mute">`;
    d.onclick = () => { sel = t.id; refresh(); };
    const [g, m] = d.querySelectorAll("input");
    g.onclick = m.onclick = (e) => e.stopPropagation();
    g.oninput = () => apply({ cmd: "set_mix", track: t.id, gain: +g.value });
    m.onchange = () => apply({ cmd: "set_mix", track: t.id, mute: m.checked });
    $("tracks").append(d);
  }
  renderExpr(p); draw(p);
}

function renderExpr(p) {
  const t = p.tracks.find((x) => x.id === sel), box = $("expr");
  box.innerHTML = "";
  if (!t || t.kind.type !== "vocal") { box.textContent = "Select a vocal track."; return; }
  const defs = [["vibrato_depth", 0, 100, 1], ["vibrato_rate", 1, 10, 0.1], ["vibrato_delay", 0, 1, 0.05], ["portamento", 0, 0.3, 0.01], ["breath", 0, 1, 0.05], ["formant_shift", -6, 6, 0.5]];
  for (const [k, lo, hi, st] of defs) {
    const r = document.createElement("div"); r.className = "row";
    r.innerHTML = `<span>${k}</span><input type="range" min="${lo}" max="${hi}" step="${st}" value="${t.expression[k]}">`;
    r.querySelector("input").onchange = (e) => apply({ cmd: "set_expression", track: t.id, expression: { ...t.expression, [k]: +e.target.value } });
    box.append(r);
  }
}

// ---- piano roll ----
const cv = $("roll"), g = cv.getContext("2d");
function draw(p = proj()) {
  const dpr = devicePixelRatio || 1, r = cv.getBoundingClientRect();
  cv.width = r.width * dpr; cv.height = r.height * dpr; g.setTransform(dpr, 0, 0, dpr, 0, 0);
  g.fillStyle = "#0e0e10"; g.fillRect(0, 0, r.width, r.height);
  for (let n = LOW; n <= HIGH; n++) {
    const y = (HIGH - n) * KEY_H, black = [1, 3, 6, 8, 10].includes(n % 12);
    g.fillStyle = black ? "#131316" : "#17171b"; g.fillRect(KEYS_W, y, r.width, KEY_H);
    g.fillStyle = black ? "#222" : "#ddd"; g.fillRect(0, y, KEYS_W, KEY_H - 1);
    if (n % 12 === 0) { g.fillStyle = "#333"; g.font = "10px sans-serif"; g.fillText("C" + (n / 12 - 1), 6, y + 11); }
  }
  for (let t = 0; t * PX_PER_TICK < r.width; t += PPQ) {
    g.strokeStyle = t % (PPQ * 4) === 0 ? "#3a3a44" : "#222228";
    const x = KEYS_W + t * PX_PER_TICK; g.beginPath(); g.moveTo(x, 0); g.lineTo(x, r.height); g.stroke();
  }
  for (const t of p.tracks) for (const n of t.notes) {
    const x = KEYS_W + n.start * PX_PER_TICK, y = (HIGH - n.pitch) * KEY_H, w = n.duration * PX_PER_TICK;
    g.globalAlpha = t.id === sel ? 1 : 0.3; g.fillStyle = t.kind.type === "vocal" ? "#c9a24a" : "#4aa3c9";
    g.fillRect(x, y + 1, w - 1, KEY_H - 2);
    if (n.lyric) { g.fillStyle = "#000"; g.font = "11px sans-serif"; g.fillText(n.lyric, x + 3, y + 11); }
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
  if (t && note) { const l = prompt("Lyric", note.lyric ?? "la"); if (l != null) apply({ cmd: "set_lyrics", track: t.id, lyrics: t.notes.map((n) => (n.id === note.id ? l : n.lyric ?? "la")) }); }
};
cv.oncontextmenu = (e) => { e.preventDefault(); const p = proj(), { t, note } = hit(e, p); if (t && note) apply({ cmd: "remove_note", track: t.id, note: note.id }); };
addEventListener("resize", () => draw());

// ---- transport ----
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
$("add-inst").onclick = () => apply({ cmd: "add_track", name: "Instrument", kind: { type: "instrument", waveform: "pluck" } });
$("add-vocal").onclick = () => apply({ cmd: "add_track", name: "Vocal", kind: { type: "vocal", voice: "default" } });
$("run").onclick = () => {
  try { const v = JSON.parse($("cmd").value); (Array.isArray(v) ? v : [v]).forEach(apply); } catch (e) { log("ERROR: " + e); }
};
const download = (name, data, type) => { const a = document.createElement("a"); a.href = URL.createObjectURL(new Blob([data], { type })); a.download = name; a.click(); };
$("wav").onclick = () => download("kymion.wav", eng.wav(), "audio/wav");
$("save").onclick = () => download("project.kymion.json", eng.project_json(), "application/json");
$("load").onclick = () => $("file").click();
$("file").onchange = async (e) => { try { eng = KymionEngine.load(await e.target.files[0].text()); refresh(); } catch (err) { log("ERROR: " + err); } };

apply({ cmd: "add_track", name: "Piano", kind: { type: "instrument", waveform: "pluck" } });
apply({ cmd: "add_track", name: "Lead Vocal", kind: { type: "vocal", voice: "default" } });

//! WASM bindings: the same Engine the CLI uses, driven from the browser GUI.

use kymion_audio::{analyze, interleave, render, render_with, to_wav_bytes, RenderOptions};
use kymion_core::{command_schema, midi::{export_midi, import_midi}, Engine, Project};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct KymionEngine {
    inner: Engine,
}

fn err(e: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&e.to_string())
}

#[wasm_bindgen]
impl KymionEngine {
    #[wasm_bindgen(constructor)]
    pub fn new(name: &str) -> KymionEngine {
        KymionEngine {
            inner: Engine::new(Project::new(name)),
        }
    }
    pub fn load(json: &str) -> Result<KymionEngine, JsValue> {
        Ok(KymionEngine {
            inner: Engine::new(Project::from_json(json).map_err(err)?),
        })
    }
    /// Import Standard MIDI File bytes (.mid) into a new KymionEngine.
    pub fn import_midi(bytes: &[u8]) -> Result<KymionEngine, JsValue> {
        let p = import_midi(bytes).map_err(err)?;
        Ok(KymionEngine {
            inner: Engine::new(p),
        })
    }
    /// Export the current project to Standard MIDI File (.mid) bytes.
    pub fn export_midi(&self) -> Result<Vec<u8>, JsValue> {
        export_midi(&self.inner.project).map_err(err)
    }
    /// Apply one JSON command; returns the outcome JSON.
    pub fn apply(&mut self, command_json: &str) -> Result<String, JsValue> {
        let o = self.inner.apply_json(command_json).map_err(err)?;
        Ok(serde_json::to_string(&o).unwrap())
    }
    pub fn undo(&mut self) -> Result<(), JsValue> {
        self.inner.undo().map_err(err)
    }
    pub fn redo(&mut self) -> Result<(), JsValue> {
        self.inner.redo().map_err(err)
    }
    pub fn project_json(&self) -> String {
        self.inner.project.to_json()
    }
    /// Project JSON without embedded audio data (cheap for GUI refreshes).
    pub fn project_json_light(&self) -> String {
        self.inner.project.to_json_light()
    }
    /// Import mono f32 samples as an audio asset; returns the asset id.
    pub fn import_audio(&mut self, name: &str, sample_rate: u32, samples: &[f32]) -> u32 {
        self.inner.import_audio(name, sample_rate, samples)
    }
    /// Peak envelope (max |sample| per bucket) of an asset for waveform drawing.
    pub fn asset_peaks(&self, asset: u32, buckets: u32) -> Vec<f32> {
        let Some(a) = self.inner.project.assets.iter().find(|a| a.id == asset) else { return vec![] };
        let data = a.samples();
        let n = buckets.max(1) as usize;
        let step = (data.len() / n).max(1);
        (0..n)
            .map(|i| data[(i * step).min(data.len())..((i + 1) * step).min(data.len())].iter().fold(0f32, |m, v| m.max(v.abs())))
            .collect()
    }
    /// Render with a metronome click (interleaved stereo f32).
    pub fn render_opts(&self, metronome: bool) -> Vec<f32> {
        interleave(&render_with(&self.inner.project, &RenderOptions { metronome }))
    }
    pub fn sample_rate(&self) -> u32 {
        self.inner.project.sample_rate
    }
    /// Interleaved stereo f32 for WebAudio.
    pub fn render(&self) -> Vec<f32> {
        interleave(&render(&self.inner.project))
    }
    pub fn wav(&self) -> Vec<u8> {
        to_wav_bytes(&render(&self.inner.project))
    }
    pub fn analysis_json(&self) -> String {
        serde_json::to_string(&analyze(&render(&self.inner.project))).unwrap()
    }
    pub fn schema() -> String {
        command_schema()
    }
}

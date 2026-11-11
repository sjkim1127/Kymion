//! WASM bindings: the same Engine the CLI uses, driven from the browser GUI.

use kymion_audio::{analyze, interleave, render_full, to_wav_bytes, FullRender, RenderOptions};
use kymion_core::{
    command_schema,
    midi::{export_midi, import_midi},
    Engine, Project,
};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct KymionEngine {
    inner: Engine,
    cache: Option<FullRender>,
    cache_metro: bool,
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
            cache: None,
            cache_metro: false,
        }
    }
    pub fn load(json: &str) -> Result<KymionEngine, JsValue> {
        Ok(KymionEngine {
            inner: Engine::new(Project::from_json(json).map_err(err)?),
            cache: None,
            cache_metro: false,
        })
    }
    /// Import Standard MIDI File bytes (.mid) into a new KymionEngine.
    pub fn import_midi(bytes: &[u8]) -> Result<KymionEngine, JsValue> {
        let p = import_midi(bytes).map_err(err)?;
        Ok(KymionEngine {
            inner: Engine::new(p),
            cache: None,
            cache_metro: false,
        })
    }
    /// Export the current project to Standard MIDI File (.mid) bytes.
    pub fn export_midi(&self) -> Result<Vec<u8>, JsValue> {
        export_midi(&self.inner.project).map_err(err)
    }
    /// Apply one JSON command; returns the outcome JSON.
    pub fn apply(&mut self, command_json: &str) -> Result<String, JsValue> {
        let o = self.inner.apply_json(command_json).map_err(err)?;
        self.cache = None;
        Ok(serde_json::to_string(&o).unwrap())
    }
    pub fn undo(&mut self) -> Result<(), JsValue> {
        self.inner.undo().map_err(err)?;
        self.cache = None;
        Ok(())
    }
    pub fn redo(&mut self) -> Result<(), JsValue> {
        self.inner.redo().map_err(err)?;
        self.cache = None;
        Ok(())
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
        let id = self.inner.import_audio(name, sample_rate, samples);
        self.cache = None;
        id
    }
    /// Import SoundFont 2 (.sf2) binary bytes as a soundfont asset; returns the asset id.
    pub fn import_soundfont(&mut self, name: &str, bytes: &[u8]) -> u32 {
        let id = self.inner.import_soundfont(name, bytes);
        self.cache = None;
        id
    }
    /// Return JSON array of presets available in an imported SoundFont asset: [{"bank":0,"preset":0,"name":"..."}]
    pub fn soundfont_presets(&self, asset: u32) -> String {
        let Some(sf) = self.inner.project.soundfonts.iter().find(|s| s.id == asset) else {
            return "[]".to_string();
        };
        let bytes = sf.bytes();
        let list = kymion_audio::list_soundfont_presets(&bytes);
        serde_json::to_string(&list).unwrap_or_else(|_| "[]".to_string())
    }
    /// Peak envelope (max |sample| per bucket) of an asset for waveform drawing.
    pub fn asset_peaks(&self, asset: u32, buckets: u32) -> Vec<f32> {
        let Some(a) = self.inner.project.assets.iter().find(|a| a.id == asset) else {
            return vec![];
        };
        let data = a.samples();
        let n = buckets.max(1) as usize;
        let step = (data.len() / n).max(1);
        (0..n)
            .map(|i| {
                data[(i * step).min(data.len())..((i + 1) * step).min(data.len())]
                    .iter()
                    .fold(0f32, |m, v| m.max(v.abs()))
            })
            .collect()
    }
    /// Render with a metronome click (interleaved stereo f32).
    pub fn render_opts(&mut self, metronome: bool) -> Vec<f32> {
        self.ensure_render(metronome);
        interleave(&self.cache.as_ref().unwrap().master)
    }
    pub fn sample_rate(&self) -> u32 {
        self.inner.project.sample_rate
    }
    /// Interleaved stereo f32 for WebAudio.
    pub fn render(&mut self) -> Vec<f32> {
        self.render_opts(false)
    }
    pub fn wav(&mut self) -> Vec<u8> {
        self.ensure_render(false);
        to_wav_bytes(&self.cache.as_ref().unwrap().master)
    }
    pub fn analysis_json(&mut self) -> String {
        self.ensure_render(false);
        serde_json::to_string(&analyze(&self.cache.as_ref().unwrap().master)).unwrap()
    }
    pub fn schema() -> String {
        command_schema()
    }

    fn ensure_render(&mut self, metronome: bool) {
        if self.cache.is_none() || self.cache_metro != metronome {
            self.cache = Some(render_full(
                &self.inner.project,
                &RenderOptions { metronome },
            ));
            self.cache_metro = metronome;
        }
    }

    /// Render a slice of interleaved stereo audio starting at `start_sample` for `num_samples`,
    /// wrapping seamlessly if a loop region is set.
    pub fn render_slice(
        &mut self,
        start_sample: u32,
        num_samples: u32,
        metronome: bool,
    ) -> Vec<f32> {
        self.ensure_render(metronome);
        let full = self.cache.as_ref().unwrap();
        let ml = &full.master.left;
        let mr = &full.master.right;
        let total = ml.len();
        let sr = self.inner.project.sample_rate as f32;

        let loop_bounds = self.inner.project.loop_region.and_then(|lr| {
            let s = (self.inner.project.ticks_to_secs(lr.start) * sr) as usize;
            let e = (self.inner.project.ticks_to_secs(lr.end) * sr) as usize;
            if e > s {
                Some((s, e))
            } else {
                None
            }
        });

        let mut out = Vec::with_capacity(num_samples as usize * 2);
        for i in 0..num_samples as usize {
            let mut pos = start_sample as usize + i;
            if let Some((ls, le)) = loop_bounds {
                if pos >= le {
                    pos = ls + (pos - ls) % (le - ls);
                }
            }
            if pos < total {
                out.push(ml[pos]);
                out.push(mr[pos]);
            } else {
                out.push(0.0);
                out.push(0.0);
            }
        }
        out
    }

    /// Calculate RMS and peak meters for master and all tracks at the given sample slice.
    pub fn meter_levels(&mut self, start_sample: u32, num_samples: u32) -> String {
        self.ensure_render(false);
        let m = self
            .cache
            .as_ref()
            .unwrap()
            .meters_at(start_sample as usize, num_samples as usize);
        serde_json::to_string(&m).unwrap_or_else(|_| "{}".into())
    }
}

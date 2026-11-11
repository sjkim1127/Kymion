// Kymion AudioWorkletProcessor — high-priority audio hardware rendering thread.
// Plays continuous audio from a ring buffer fed by the main WASM thread,
// and streams accurate playhead timestamps and peak/RMS levels back.

class KymionWorkletProcessor extends AudioWorkletProcessor {
  constructor() {
    super();
    this.capacity = 65536; // ~1.48 seconds ring buffer at 44.1kHz
    this.ringL = new Float32Array(this.capacity);
    this.ringR = new Float32Array(this.capacity);
    this.writePos = 0;
    this.readPos = 0;
    this.totalSamplesPlayed = 0;
    this.playing = false;
    this.statusCounter = 0;

    this.port.onmessage = (e) => {
      const msg = e.data;
      if (!msg) return;
      if (msg.type === "push") {
        const data = msg.data;
        const n = (data.length / 2) | 0;
        for (let i = 0; i < n; i++) {
          const idx = (this.writePos + i) % this.capacity;
          this.ringL[idx] = data[2 * i];
          this.ringR[idx] = data[2 * i + 1];
        }
        this.writePos = (this.writePos + n) % this.capacity;
      } else if (msg.type === "clear") {
        this.writePos = 0;
        this.readPos = 0;
        this.ringL.fill(0);
        this.ringR.fill(0);
      } else if (msg.type === "set_playing") {
        this.playing = !!msg.playing;
        if (msg.totalSamples != null) {
          this.totalSamplesPlayed = msg.totalSamples;
        }
      }
    };
  }

  process(inputs, outputs, parameters) {
    const out = outputs[0];
    if (!out || out.length === 0) return true;
    const outL = out[0];
    const outR = out[1] || outL;
    const blockSize = outL.length;

    let peakL = 0, peakR = 0, sumL = 0, sumR = 0;

    if (!this.playing) {
      outL.fill(0);
      if (out[1]) outR.fill(0);
    } else {
      for (let i = 0; i < blockSize; i++) {
        if (this.readPos !== this.writePos) {
          const l = this.ringL[this.readPos];
          const r = this.ringR[this.readPos];
          outL[i] = l;
          if (out[1]) outR[i] = r;
          this.readPos = (this.readPos + 1) % this.capacity;
          this.totalSamplesPlayed++;

          const al = Math.abs(l), ar = Math.abs(r);
          if (al > peakL) peakL = al;
          if (ar > peakR) peakR = ar;
          sumL += l * l;
          sumR += r * r;
        } else {
          outL[i] = 0;
          if (out[1]) outR[i] = 0;
        }
      }
    }

    this.statusCounter++;
    if (this.statusCounter >= 8) {
      this.statusCounter = 0;
      const buffered =
        this.writePos >= this.readPos
          ? this.writePos - this.readPos
          : this.capacity - this.readPos + this.writePos;

      this.port.postMessage({
        type: "status",
        totalSamples: this.totalSamplesPlayed,
        buffered,
        playing: this.playing,
        peakL,
        peakR,
        rmsL: Math.sqrt(sumL / blockSize),
        rmsR: Math.sqrt(sumR / blockSize),
      });
    }

    return true;
  }
}

registerProcessor("kymion-worklet-processor", KymionWorkletProcessor);

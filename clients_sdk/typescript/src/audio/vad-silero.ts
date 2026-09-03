/**
 * Opt-in ONNX Silero VAD (D3b).
 *
 * The default VAD is the energy gate in `vad.ts` (zero deps). Silero is a small
 * neural VAD that is far more robust in noise, but it needs `onnxruntime-web`
 * (a multi-MB WASM dep) + the Silero model weights, so it is STRICTLY OPT-IN and
 * lazily loaded — nothing here is imported unless you construct a `SileroVAD`.
 *
 * It follows the documented Silero streaming contract: 512-sample frames at
 * 16kHz, RNN hidden state carried frame-to-frame, and a PERIODIC STATE RESET
 * (every ~5s) so the recurrent state can't drift/wedge on a long stream. The
 * heavy `onnxruntime-web` import is dynamic + guarded: if it (or the model) is
 * unavailable the detector reports "unavailable" and the caller falls back to
 * the energy path rather than throwing.
 *
 * To keep the SDK dependency-free, `onnxruntime-web` is NOT a hard dependency;
 * provide a session factory (or rely on the dynamic import when the host app has
 * it installed). The class is intentionally a thin, well-documented seam: it
 * runs the model when present and degrades cleanly when not, so the energy
 * default keeps working everywhere.
 */

/** Tunables for the Silero VAD. */
export interface SileroVADConfig {
  /** Model sample rate (Silero supports 8k/16k; default 16000). */
  sampleRate?: 8000 | 16000;
  /**
   * Speech-probability threshold (0..1, default 0.5). Frames above this are
   * speech. Silero's probability is already noise-robust, so a single threshold
   * is fine here (the energy path is what needs the dB hysteresis).
   */
  threshold?: number;
  /**
   * Reset the recurrent hidden state every this many ms (default 5000) so it
   * can't drift on a long stream. Per the Silero streaming guidance.
   */
  resetIntervalMs?: number;
  /**
   * URL of the Silero ONNX model. Required to actually run; when omitted the
   * detector stays unavailable (energy fallback). Point at your bundled
   * `silero_vad.onnx`.
   */
  modelUrl?: string;
  /**
   * Optional factory that creates an inference session, so the host can wire its
   * own `onnxruntime-web` build/Worker. Receives the resolved `modelUrl`. When
   * absent, a dynamic `import('onnxruntime-web')` is attempted.
   */
  createSession?: (modelUrl: string) => Promise<SileroSession>;
}

/** The minimal ONNX session surface the Silero runner needs. */
export interface SileroSession {
  /** Run one inference; returns the named output tensors' data. */
  run(feeds: Record<string, SileroTensor>): Promise<Record<string, { data: Float32Array | ArrayLike<number> }>>;
}

/** A minimal tensor shape compatible with onnxruntime-web's `Tensor`. */
export interface SileroTensor {
  type: string;
  data: Float32Array | BigInt64Array;
  dims: number[];
}

/** The fixed Silero frame size at each rate (samples). */
function frameSamplesFor(rate: 8000 | 16000): number {
  return rate === 16000 ? 512 : 256;
}

/**
 * Lazily-loaded Silero VAD runner. Construct it (cheap), call `init()` once
 * (loads the model — may resolve to unavailable), then feed exact 512@16k
 * frames to `process()` to get a speech probability. The energy VAD in `vad.ts`
 * can delegate to this when `vadEngine: 'silero'` is requested, falling back to
 * energy if `isAvailable()` is false.
 */
export class SileroVAD {
  private readonly config: Required<Omit<SileroVADConfig, 'createSession'>> & Pick<SileroVADConfig, 'createSession'>;
  private session: SileroSession | null = null;
  private available = false;
  private initialized = false;
  private readonly frameSamples: number;

  // Recurrent state (Silero v4 uses a single combined `state` tensor; v3 uses
  // h/c). We carry an opaque state buffer and reset it periodically.
  private state: Float32Array;
  private lastReset = 0;

  constructor(config: SileroVADConfig = {}) {
    const sampleRate = config.sampleRate ?? 16000;
    this.config = {
      sampleRate,
      threshold: config.threshold ?? 0.5,
      resetIntervalMs: config.resetIntervalMs ?? 5000,
      modelUrl: config.modelUrl ?? '',
      ...(config.createSession ? { createSession: config.createSession } : {}),
    };
    this.frameSamples = frameSamplesFor(sampleRate);
    // Silero v4 state tensor shape is [2, 1, 128].
    this.state = new Float32Array(2 * 1 * 128);
  }

  /** The exact frame size (samples) this VAD expects (512 @16k / 256 @8k). */
  getFrameSamples(): number {
    return this.frameSamples;
  }

  /** Whether the model loaded and inference is available (else use energy). */
  isAvailable(): boolean {
    return this.available;
  }

  /**
   * Load the model. Resolves whether Silero is available; on failure (no
   * onnxruntime-web, no modelUrl, load error) it resolves false and the caller
   * should fall back to the energy VAD. Safe to call once; idempotent.
   */
  async init(): Promise<boolean> {
    if (this.initialized) return this.available;
    this.initialized = true;
    if (!this.config.modelUrl) return false;
    try {
      if (this.config.createSession) {
        this.session = await this.config.createSession(this.config.modelUrl);
      } else {
        this.session = await this.defaultCreateSession(this.config.modelUrl);
      }
      this.available = this.session !== null;
    } catch {
      this.available = false;
    }
    return this.available;
  }

  /**
   * Run one 512@16k (or 256@8k) frame and return the speech probability (0..1),
   * or null when unavailable / wrong frame size. Carries + periodically resets
   * the recurrent state.
   */
  async process(frame: Float32Array, nowMs: number): Promise<number | null> {
    if (!this.available || !this.session) return null;
    if (frame.length !== this.frameSamples) return null;

    if (nowMs - this.lastReset >= this.config.resetIntervalMs) {
      this.resetState();
      this.lastReset = nowMs;
    }

    const sr = this.config.sampleRate;
    try {
      const out = await this.session.run({
        input: { type: 'float32', data: Float32Array.from(frame), dims: [1, frame.length] },
        sr: { type: 'int64', data: BigInt64Array.from([BigInt(sr)]), dims: [] },
        state: { type: 'float32', data: Float32Array.from(this.state), dims: [2, 1, 128] },
      });
      // Carry the updated recurrent state forward (key name is model-dependent).
      const newState = out.stateN ?? out.state;
      if (newState && newState.data) {
        this.state = Float32Array.from(newState.data as ArrayLike<number>);
      }
      const prob = out.output?.data;
      if (prob && prob.length > 0) {
        return Number(prob[0]);
      }
      return null;
    } catch {
      // A runtime failure disables Silero for the rest of the session.
      this.available = false;
      return null;
    }
  }

  /** Whether a probability counts as speech given the configured threshold. */
  isSpeech(prob: number): boolean {
    return prob >= this.config.threshold;
  }

  /** Reset the recurrent hidden state (also called periodically by process()). */
  resetState(): void {
    this.state = new Float32Array(this.state.length);
  }

  /**
   * Default session factory: dynamically import `onnxruntime-web` (guarded so the
   * default bundle never depends on it) and create an inference session. Returns
   * null if the dependency isn't installed.
   */
  private async defaultCreateSession(modelUrl: string): Promise<SileroSession | null> {
    try {
      // Indirect specifier so bundlers don't try to resolve onnxruntime-web into
      // the SDK bundle — it is an OPTIONAL peer the host app provides.
      const ortName = 'onnxruntime-web';
      const ort = (await import(/* @vite-ignore */ ortName)) as {
        InferenceSession?: { create(url: string): Promise<SileroSession> };
      };
      if (!ort.InferenceSession) return null;
      return await ort.InferenceSession.create(modelUrl);
    } catch {
      return null;
    }
  }
}

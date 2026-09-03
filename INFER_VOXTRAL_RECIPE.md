# Voxtral-Mini-4B-Realtime-2602 — ONNX decode recipe (for the `voxtral_realtime` arm)

Ref impl + configs at /tmp/voxtral/ (antirez_python_simple_implementation.py is the 1:1 reference).
onnx-community/Voxtral-Mini-4B-Realtime-2602-ONNX. 3 graphs: audio_encoder + embed_tokens + decoder_model_merged.

**KEY:** NOT Llava-scatter. It's a LOCKSTEP streaming transcriber — audio_embeds are ELEMENT-WISE ADDED to
text-token embeds at the same position; decoder emits exactly ONE text token per audio token. Token [AUDIO]=24
is a 1:1 marker, splice by SUMMATION not replacement.

Arch: decoder 26L hidden3072 32Q/8KV heads head_dim128 SwiGLU RMSNorm vocab131072 tied; audio_enc 32L hidden1280
32heads head_dim64 128-mel; bridge downsample4 audio_len_per_tok8 default_num_delay_tokens6.

A. Mel: Whisper STFT (n_fft400 hop160 hann center, 128 mel slaney 0-8000) BUT fixed normalization:
   log10(clamp(mel,1e-10)); log=max(log, GLOBAL_LOG_MEL_MAX(=1.5) - 8.0); (log+4)/4. (NOT per-clip max!)
   Pad audio: left=32*1280=40960, right=align+(17*1280); run mel on padded. If mel T odd, left-trim 1 frame.
B. audio_encoder offline: run ONCE, past_kv zeros[1,32,0,64], past_padding_cache ZEROS[1,1408,2],
   position_ids=arange(post-conv len = num_audio_tokens*4), attn_mask ones. Read audio_embeds[1,N,3072];
   discard present.*. num_audio_tokens ≈ T_mel/8 (12.5Hz, one token/80ms). 32 enc layers.
C. Prompt (offline): [BOS=1] + [STREAMING_PAD=32]*(32+6) → L=39. NO [AUDIO]/[BEGIN_AUDIO] in realtime path.
   inputs_embeds[L] = audio_embeds[:L] + embed_tokens(prompt_ids)  (SUM). Need L<=N.
D. Decode (lockstep): prefill inputs_embeds[L] empty past → argmax logits[-1] = first token. Then for pos in L..N:
   step_embed = audio_embeds[pos] + embed_tokens(token); attn_mask ones[pos+1]; past=present; argmax; append.
   Stop EOS=2; max tokens = num_audio_tokens. 26 decoder layers, greedy. **NOTE: decoder graph has 480ms delay
   (num_delay_tokens=6) BAKED IN as constants (no t_cond input) — locked to that delay.**
E. Tokenizer: tekken v7. Use the repo's tokenizer.json (HF, Rust `tokenizers` crate loadable) for decode; skip
   ids<1000 (specials). (Raw tekken: vocab[id-1000].token_bytes b64 — the +1000 offset gotcha.)
F. 13 langs auto-detected (no lang token offline). Output = decode + strip.
Files to fetch (REPO ROOT): config.json generation_config.json preprocessor_config.json tekken.json
tokenizer.json; graphs under onnx/. special_tokens_map.json does NOT exist (specials in tekken.json).

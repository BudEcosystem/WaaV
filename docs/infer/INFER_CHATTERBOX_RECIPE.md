# Chatterbox / Chatterbox-Turbo — portable ONNX recipe (new `chatterbox` TTS arm)
Full extract: agent transcript. Source copies /tmp/chatterbox/. PORTABLE PATH = official ONNX, NO venv/pip.

**Official ONNX (use as-is, directive #3):** onnx-community/chatterbox-ONNX (base, +multilingual),
ResembleAI/chatterbox-turbo-ONNX (turbo, fp16/q4/q4f16/quantized). 4-graph decomposition (both):
- speech_encoder.onnx: audio_values → cond_emb, prompt_token, ref_x_vector, prompt_feat  (VoiceEncoder+
  S3Tokenizer+CAMPPlus+ref-mel; SKIP for fixed voice if shipping conds.pt).
- embed_tokens.onnx: input_ids, position_ids, exaggeration → inputs_embeds  (T3 text/speech emb + cond prefix).
- language_model.onnx: inputs_embeds, attention_mask, past_kv[60] → logits, past_kv[60]  (T3 backbone +
  speech_head; 30 layers×2, kv_heads 16, head_dim 64). **AR loop runs in OUR host code (AR-decoder seam).**
- conditional_decoder.onnx: speech_tokens, speaker_embeddings, speaker_features → 24kHz wav  (S3Gen conformer
  + CFM[Euler/MeanFlow rolled in] + HiFTGenerator). One call.

**AR loop (host, reuse seam):** embed_tokens once for prefix → iterate language_model w/ KV cache.
- BASE sampling: rep_penalty(1.2) → temp(0.8) → min_p(0.05) → top_p(1.0) → CFG combine(cfg_weight 0.5,
  duplicate batch, uncond text zeroed) → multinomial. BOS=6561, stop EOS=6562, keep tok<6561.
- TURBO sampling: temp(0.8)→top_k(1000)→top_p(0.95)→rep(1.2), NO CFG, NO learned pos emb; append 3×SIL(4299).
Both: max 1000 tokens. S3GEN_SR=24000.
**Config flag base|turbo:** backbone Llama_520M(30L,RoPE)|GPT2_medium(24L,absPE); perceiver/emotion/CFG yes|no;
CFM cosine-Euler-10+CFG(0.7) | linear-MeanFlow-2step(noised_mels init). One arm serves both via the flag.
Voice: ship/precompute conds.pt for default voice → skip speech_encoder. Watermark (Perth) = post-hoc, SKIP.
Verify: ASR round-trip (output → funasr/nemotron transcribe == input text), like the original chatterbox check.

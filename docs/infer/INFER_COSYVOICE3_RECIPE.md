# CosyVoice3-0.5B — portable inference recipe (next TTS arm; torch sidecar P4, shared env)
Source copies /tmp/cosyvoice/. 4-stage zero-shot-cloning TTS @24kHz. Cloning-ONLY (no SFT speaker) → ship a
default reference + PRECOMPUTE its (prompt_speech_token, spk192, prompt_feat, prompt_text_ids).

PIPELINE: text→Qwen2-BPE ids; ref→[S3-tokenizer.onnx]→prompt_tok, [campplus.onnx]→spk192, [matcha-mel]→prompt_feat.
[A] LLM (llm.rl.pt = Qwen2 0.5B body[BlankEN, hid896/24L/14h/2kv] + CUSTOM speech_embedding[6761,896] + llm_decoder
    Linear896→6761; NOT Qwen LM head). Prompt emb = cat[speech_emb[sos=6561], qwen.embed_tokens(cat[prompt_text,
    tts_text]), speech_emb[task=6563], speech_emb(prompt_tok)]. AR KV-cached loop, RAS sampling (ras_sampling: top_p
    0.8 top_k 25 win10 tau_r 0.1; common.py), stop id>=6561(eos6562); min=2*len(tts) max=20*len(tts); mask eos while
    i<min. <|endofprompt|>=151646 MUST be in [prompt_text+tts_text]. prompt_text="You are a helpful assistant.
    <|endofprompt|>"+ref_transcript. Feed token emb back: speech_embedding[tid]. (spk192 NOT fed to LLM.)
[B] Flow CausalMaskedDiffWithDiT (NO conformer encoder): spk80=Linear192→80(F.normalize(spk192)); tok=cat[prompt_tok,
    speech_tokens]; h=pre_lookahead(input_embedding[6561,80](tok)*mask) [2 causal convs]; h=repeat_interleave(2)
    (25→50Hz); cond=zeros[T,80] w/ first mel_len1=prompt_feat; CFM solve_euler n_timesteps10 cosine t_span=1-cos(lin*
    pi/2); DETERMINISTIC noise z=randn[1,80,15000] seed0 [:,:,:T]; CFG batch=2 (row0 cond, row1 ZEROS) dphi=1.7*c-0.7*u
    rate0.7; x+=dt*dphi. ESTIMATOR = flow.decoder.estimator.fp32.onnx (REUSE; in x/mask/mu/t/spks/cond all [2,80,T]/
    [2]/[2,80], out [2,80,T], batch FIXED 2). mel=x[:,:,mel_len1:].
[C] HiFT CausalHiFTGenerator (hift.pt, VENDOR ~3 classes: generator+f0_predictor+SourceModuleHnNSF; NO onnx): NSF+
    iSTFT, f0_predictor runs FLOAT64 (precision-critical). mel→24kHz wav. inference(mel, finalize=True).
PORTABLE DECOMP: S3+campplus+estimator = ONNX/ORT; LLM = transformers Qwen2 + custom heads (shared env, like ARK/
granite); flow-wrapper + HiFT = vendor architecture-only (~3 classes, NO cosyvoice pip pkg at serving). All dims from
cosyvoice3.yaml (config-driven). use llm.rl.pt (RL-finetuned, better CER) not llm.pt.

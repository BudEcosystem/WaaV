#!/usr/bin/env python3
"""Deepgram streaming STT finalization-latency probe.

Measures the realtime-critical number: after the user stops speaking
(end-of-speech), how long until WaaV has the FINAL transcript it can hand
to the LLM. Streams real Aura-synthesized PCM at 1x (realtime) pace, marks
the last-audio-sent instant as end-of-speech, and times each Deepgram event
relative to it. Keys are read from env only.
"""
import asyncio, json, os, time, sys, urllib.request
import websockets

DG = os.environ["DG_KEY"]
SR = 16000
UTTERANCE = "Hello, what time should I leave for my nine a.m. meeting downtown?"

def synth(text):
    url = (f"https://api.deepgram.com/v1/speak?model=aura-asteria-en"
           f"&encoding=linear16&container=none&sample_rate={SR}")
    req = urllib.request.Request(url, data=json.dumps({"text": text}).encode(),
        headers={"Authorization": f"Token {DG}", "Content-Type": "application/json"})
    return urllib.request.urlopen(req, timeout=30).read()

async def measure(pcm, endpointing, realtime=True):
    url = (f"wss://api.deepgram.com/v1/listen?model=nova-2&encoding=linear16"
           f"&sample_rate={SR}&channels=1&interim_results=true"
           f"&endpointing={endpointing}&utterance_end_ms=1000&punctuate=true&vad_events=true")
    async with websockets.connect(url, additional_headers={"Authorization": f"Token {DG}"}) as ws:
        ev = {"first_partial": None, "final": None, "speech_final": None, "utt_end": None}
        transcript = [""]
        last_sent = [None]
        async def recv():
            async for msg in ws:
                d = json.loads(msg); t = time.monotonic(); typ = d.get("type")
                if typ == "Results":
                    alt = d["channel"]["alternatives"][0]; txt = alt.get("transcript", "")
                    if txt and ev["first_partial"] is None: ev["first_partial"] = t
                    if d.get("is_final") and txt:
                        transcript[0] = txt
                        if ev["final"] is None: ev["final"] = t
                    if d.get("speech_final"): ev["speech_final"] = t
                elif typ == "UtteranceEnd":
                    if ev["utt_end"] is None: ev["utt_end"] = t
        rt = asyncio.create_task(recv())
        chunk = int(SR * 2 * 0.02)  # 20 ms of linear16
        for i in range(0, len(pcm), chunk):
            await ws.send(pcm[i:i+chunk])
            if realtime: await asyncio.sleep(0.02)
        last_sent[0] = time.monotonic()
        await ws.send(json.dumps({"type": "CloseStream"}))
        try: await asyncio.wait_for(rt, timeout=6)
        except asyncio.TimeoutError: pass
        las = last_sent[0]
        rel = lambda x: None if x is None else round((x - las) * 1000, 1)
        return {
            "endpointing_ms": endpointing,
            "audio_dur_s": round(len(pcm) / 2 / SR, 2),
            "first_partial_rel_eos_ms": rel(ev["first_partial"]),   # negative = during speech
            "is_final_after_eos_ms": rel(ev["final"]),
            "speech_final_after_eos_ms": rel(ev["speech_final"]),   # THE finalization latency
            "utterance_end_after_eos_ms": rel(ev["utt_end"]),
            "transcript": transcript[0][:80],
        }

async def main():
    pcm = synth(UTTERANCE)
    print(f"# synth: {len(pcm)} bytes = {len(pcm)/2/SR:.2f}s of speech\n")
    for ep in (10, 100, 300):
        for n in range(3):
            try:
                r = await measure(pcm, ep, realtime=True)
                print(json.dumps(r))
            except Exception as e:
                print(f"ERR ep={ep}: {e}", file=sys.stderr)
            await asyncio.sleep(0.3)

asyncio.run(main())

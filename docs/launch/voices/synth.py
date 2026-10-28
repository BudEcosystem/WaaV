"""Synthesise the same lines in many Deepgram Aura-2 voices through the local gateway's /speak."""
import json, sys, wave
from pathlib import Path
from urllib.request import Request, urlopen

GW = "http://127.0.0.1:4410/speak"
VOICES = ["helena", "andromeda", "vesta", "juno", "luna", "minerva", "cora", "cordelia", "delia", "athena", "hera",
          "harmonia", "pandora", "thalia", "arcas", "apollo", "aries", "orion", "pluto", "odysseus", "orpheus", "draco",
          "mars", "hermes"]
LINES = {
    "agent": "Good news: the terminal's back. The ARM build has twelve downloads, the x86 build none yet, and the "
             "checksums file fourteen. Want me to save the announcement now?",
    "vo": "A small front model decides every turn of the conversation, in real time. The real work runs in the "
          "background, on any model or agent you choose. And before anything changes, it asks.",
}
out = Path(sys.argv[1]); out.mkdir(parents=True, exist_ok=True)
for v in VOICES:
    for key, text in LINES.items():
        f = out / f"{key}_{v}.wav"
        if f.exists():
            continue
        body = json.dumps({"text": text, "tts_config": {"provider": "deepgram", "model": f"aura-2-{v}-en",
                           "voice_id": f"aura-2-{v}-en", "audio_format": "linear16", "sample_rate": 24000}}).encode()
        pcm = urlopen(Request(GW, data=body, headers={"content-type": "application/json"}), timeout=90).read()
        with wave.open(str(f), "wb") as w:
            w.setnchannels(1); w.setsampwidth(2); w.setframerate(24000); w.writeframes(pcm)
        print(f.name, len(pcm) / 48000, "s", flush=True)

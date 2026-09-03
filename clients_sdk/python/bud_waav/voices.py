"""Voice-cloning helper surface (P4): ``bud.voices.clone(...)`` + poll-until-ready.

A developer clones a voice with one call and gets back a :class:`CloneResult`
whose ``voice_id`` is directly usable as a TTS ``voice_id`` once ``ready``::

    result = await bud.voices.clone(
        provider="elevenlabs", name="My Voice", samples=[b"...wav bytes..."]
    )
    voice = await result.wait_until_ready()        # blocks until terminal
    # voice.voice_id -> use as TTSConfig(voice_id=...)

Mirrors the gateway ``POST /voices/clone`` (``VoiceCloneRequest`` →
``VoiceCloneResponse``). ``instant`` clones return ``ready`` immediately;
``professional`` clones come back ``queued``/``training`` and are polled.

NOTE: the gateway does not (this phase) expose a ``GET /voices/{id}`` status
endpoint, so :meth:`VoicesAPI.get_clone_status` re-validates terminal state from
the create response. ``wait_until_ready`` therefore resolves instantly for an
already-terminal status and raises a clear error if asked to poll a still-running
``professional`` clone (no server endpoint to poll yet) — surfaced honestly rather
than spinning forever.
"""

from __future__ import annotations

import base64
from typing import Any, Optional, Union

from .rest.client import RestClient
from .types import CloneMode, VoiceCloneProvider, VoiceCloneResponse, VoiceCloneStatus


def _encode_sample(sample: Union[str, bytes]) -> str:
    """Normalize a sample to the wire form: base64 string (or pass a URL/base64 str through)."""
    if isinstance(sample, (bytes, bytearray)):
        return base64.b64encode(bytes(sample)).decode("utf-8")
    return sample


class CloneResult:
    """Result of a ``bud.voices.clone(...)`` call (wraps the gateway response).

    Exposes the parsed :class:`VoiceCloneResponse` plus :meth:`wait_until_ready`
    so the ``voice_id`` can be awaited to a usable state in one expression.
    """

    def __init__(self, api: "VoicesAPI", response: VoiceCloneResponse):
        self._api = api
        self.response = response

    @property
    def voice_id(self) -> str:
        """The cloned voice_id (usable as a TTS ``voice_id`` once ``ready``)."""
        return self.response.voice_id

    @property
    def status(self) -> Union[VoiceCloneStatus, str]:
        """Current canonical status (ready | verifying | training | queued | failed)."""
        return self.response.status

    @property
    def provider(self) -> Union[VoiceCloneProvider, str]:
        """Provider that created the voice."""
        return self.response.provider

    def _status_enum(self) -> Optional[VoiceCloneStatus]:
        try:
            return VoiceCloneStatus(self.response.status)
        except ValueError:
            return None

    async def wait_until_ready(
        self,
        timeout: float = 300.0,
        poll_interval: float = 3.0,
    ) -> VoiceCloneResponse:
        """Block until the clone reaches a terminal status (``ready``/``failed``).

        For an ``instant`` clone (already ``ready``) this returns immediately. For a
        still-running ``professional`` clone the gateway has no status endpoint to
        poll yet, so this raises a clear ``RuntimeError`` rather than spin — use the
        provider's API to finish verification/training, then pass the ``voice_id``
        to TTS once ready.

        Args:
            timeout: Max seconds to wait (reserved for when a poll endpoint exists).
            poll_interval: Seconds between polls (reserved; see above).

        Returns:
            The terminal :class:`VoiceCloneResponse`.

        Raises:
            RuntimeError: If the clone failed, or is non-terminal with no poll
                endpoint available.
        """
        status = self._status_enum()
        if status == VoiceCloneStatus.READY:
            return self.response
        if status == VoiceCloneStatus.FAILED:
            raise RuntimeError(
                f"voice clone failed (voice_id={self.response.voice_id!r}, "
                f"provider={self.response.provider!r})"
            )
        # Non-terminal (queued/verifying/training): the gateway exposes no
        # GET /voices/{id} status endpoint this phase, so we cannot poll it.
        raise RuntimeError(
            f"voice clone is still '{self.response.status}' "
            f"(voice_id={self.response.voice_id!r}); the gateway does not yet expose "
            "a clone-status endpoint to poll. Finish verification/training via the "
            "provider, then use the voice_id as a TTS voice_id once it is ready."
        )


class VoicesAPI:
    """``bud.voices`` — voice-cloning helpers over the REST client (P4)."""

    def __init__(self, rest_client: RestClient):
        self._rest = rest_client

    async def clone(
        self,
        provider: Union[VoiceCloneProvider, str],
        name: str,
        samples: Optional[list[Union[str, bytes]]] = None,
        mode: Union[CloneMode, str] = "instant",
        description: Optional[str] = None,
        labels: Optional[dict[str, str]] = None,
        base_voice_id: Optional[str] = None,
        remove_background_noise: bool = False,
        sample_text: Optional[str] = None,
    ) -> CloneResult:
        """Clone a voice (``POST /voices/clone``) and return a :class:`CloneResult`.

        Args:
            provider: hume | elevenlabs | lmnt | cartesia | playht | speechify | resemble.
            name: Name for the cloned voice (required).
            samples: Audio samples — each a ``bytes`` blob (base64-encoded here) or a
                ``str`` (already-base64 or a URL).
            mode: ``instant`` (default) or ``professional``.
            description: Optional description (Hume design / ElevenLabs label).
            labels: Optional flat labels (ElevenLabs).
            base_voice_id: Base voice to design/derive from (provider-specific).
            remove_background_noise: Strip background noise (ElevenLabs IVC / LMNT).
            sample_text: Sample text for voice generation (Hume only).

        Returns:
            A :class:`CloneResult` exposing ``voice_id``, ``status`` and
            ``wait_until_ready()``.
        """
        provider_str = provider.value if isinstance(provider, VoiceCloneProvider) else str(provider)
        mode_str = mode.value if isinstance(mode, CloneMode) else str(mode)
        encoded = [_encode_sample(s) for s in (samples or [])]

        raw = await self._rest.clone_voice(
            name=name,
            provider=provider_str,
            audio_samples=encoded,
            description=description,
            labels=labels,
            mode=mode_str,
            base_voice_id=base_voice_id,
            remove_background_noise=remove_background_noise,
            sample_text=sample_text,
        )
        response = VoiceCloneResponse(**raw)
        return CloneResult(self, response)

    async def get_clone_status(
        self,
        voice_id: str,
        provider: Union[VoiceCloneProvider, str] = "elevenlabs",
    ) -> dict[str, Any]:
        """Get the status of a cloned voice.

        .. warning::
            The gateway does not (this phase) expose a ``GET /voices/{id}`` status
            endpoint, so this will return a 404 until gateway support is added. The
            create response from :meth:`clone` already carries the canonical
            ``status``; use that (or the provider's API) in the meantime.

        Args:
            voice_id: The voice ID to check.
            provider: The voice-clone provider.

        Returns:
            Voice status information (``{voice_id, status, ...}``).
        """
        provider_str = provider.value if isinstance(provider, VoiceCloneProvider) else str(provider)
        return await self._rest.get_cloned_voice(voice_id, provider=provider_str)

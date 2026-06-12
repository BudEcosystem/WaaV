//! STT result processing with timing control

use parking_lot::RwLock as SyncRwLock;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio::time::Duration;
use tracing::{debug, info};

use crate::core::{stt::STTResult, turn_detect::TurnDetector};

/// Append `fragment` to `buf`, inserting a single space when both sides have
/// content and neither boundary char is whitespace — so multi-final segments
/// don't concatenate as `"Hello there.How are you"`.
fn append_with_space(buf: &mut String, fragment: &str) {
    if fragment.is_empty() {
        return;
    }
    if !buf.is_empty()
        && !buf.ends_with(char::is_whitespace)
        && !fragment.starts_with(char::is_whitespace)
    {
        buf.push(' ');
    }
    buf.push_str(fragment);
}

use super::state::SpeechFinalState;

/// Case-folded, punctuation-stripped word tokens (dedup normalization).
fn normalize_words(s: &str) -> Vec<String> {
    s.split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

/// Clamp the process-relative monotonic clock away from the RESERVED 0
/// sentinel ("unset") used by `segment_start_ms` / `last_fired_ms` /
/// `hard_timeout_deadline_ms`. In the process's first millisecond the raw
/// clock IS 0 — a segment armed then would read as "no active segment" on
/// the next is_final and silently RESTART the hard-timeout backstop.
#[inline]
fn clamp_clock(raw_ms: usize) -> usize {
    raw_ms.max(1)
}

/// Configuration for STT result processing
#[derive(Clone, Copy)]
pub struct STTProcessingConfig {
    /// Time to wait for STT provider to send real speech_final (ms)
    /// This is the primary window - we trust STT provider during this time
    pub stt_speech_final_wait_ms: u64,
    /// Maximum time to wait for turn detection inference to complete (ms)
    pub turn_detection_inference_timeout_ms: u64,
    /// Hard upper bound timeout for any user utterance (ms)
    /// This guarantees that no utterance will wait longer than this value
    /// even if neither the STT provider nor turn detector fire
    pub speech_final_hard_timeout_ms: u64,
    /// Window to prevent duplicate speech_final events (ms)
    pub duplicate_window_ms: usize,
    /// The active STT provider's measured speech-end→final p99 (ms), when
    /// known (D-G8 / A-G2). Extends the detection wait for SLOW providers so
    /// their real final isn't beaten by a forced fire (which the duplicate
    /// window then has to clean up); fast providers keep the configured
    /// user-resume floor — the floor is a UX window, never shortened by STT
    /// speed. `None` = unknown → configured wait only.
    pub stt_ttfs_p99_ms: Option<u64>,
}

impl Default for STTProcessingConfig {
    fn default() -> Self {
        Self {
            stt_speech_final_wait_ms: 1800, // Wait 1.8s for real speech_final from STT (reduced from 2s)
            turn_detection_inference_timeout_ms: 100, // 100ms max for model inference
            speech_final_hard_timeout_ms: 2500, // 2.5s hard upper bound (reduced from 5s for faster response)
            duplicate_window_ms: 500,       // 500ms duplicate prevention window
            stt_ttfs_p99_ms: None,
        }
    }
}

impl STTProcessingConfig {
    /// Create a new STTProcessingConfig with explicit timeout values
    pub fn new(
        stt_speech_final_wait_ms: u64,
        turn_detection_inference_timeout_ms: u64,
        speech_final_hard_timeout_ms: u64,
        duplicate_window_ms: usize,
    ) -> Self {
        Self {
            stt_speech_final_wait_ms,
            turn_detection_inference_timeout_ms,
            speech_final_hard_timeout_ms,
            duplicate_window_ms,
            stt_ttfs_p99_ms: None,
        }
    }

    /// Attach the provider's measured speech-end→final p99 (D-G8 / A-G2).
    pub fn with_stt_ttfs_p99_ms(mut self, ttfs: Option<u64>) -> Self {
        self.stt_ttfs_p99_ms = ttfs;
        self
    }

    /// The effective detection wait (A-G2): the configured user-resume floor,
    /// EXTENDED to the provider's TTFS p99 when that is slower — so a slow
    /// provider's real final isn't beaten by a forced fire. `finalized` =
    /// the provider acked a finalize handshake (nothing more coming): the
    /// TTFS extension collapses back to the floor. Clamped so the wait can
    /// never crowd out the hard-timeout backstop.
    pub fn effective_wait_ms(&self, finalized: bool) -> u64 {
        let floor = self.stt_speech_final_wait_ms;
        match (self.stt_ttfs_p99_ms, finalized) {
            // Only the TTFS EXTENSION is clamped (the hard-timeout backstop
            // must keep room to act). The configured floor itself keeps its
            // pre-existing semantics verbatim — A-G2 must never silently
            // shrink an operator's explicit wait (review wf_5772cd64 #11).
            (Some(ttfs), false) if ttfs > floor => {
                let clamp = ((self.speech_final_hard_timeout_ms * 2) / 3).max(floor);
                ttfs.min(clamp)
            }
            _ => floor,
        }
    }
}

/// Processor for STT results with timing control
#[derive(Clone)]
pub struct STTResultProcessor {
    config: STTProcessingConfig,
}

impl STTResultProcessor {
    pub fn new(config: STTProcessingConfig) -> Self {
        Self { config }
    }

    /// Process an STT result with timing control
    ///
    /// This method implements:
    /// - Immediate return of results (no waiting)
    /// - Turn detection ML model with intelligent timeout selection
    /// - Fast-path synchronous checks before async operations
    /// - Prevention of duplicate speech_final events
    pub async fn process_result(
        &self,
        result: STTResult,
        speech_final_state: Arc<SyncRwLock<SpeechFinalState>>,
        turn_detector: Option<Arc<RwLock<TurnDetector>>>,
    ) -> Option<STTResult> {
        // Fast synchronous checks - no awaits
        if !self.should_deliver_result(&result) {
            return None;
        }

        let now_ms = self.get_current_time_ms();

        // Handle real speech_final
        if result.is_speech_final {
            return self.handle_real_speech_final(result, speech_final_state, now_ms);
        }

        // Handle is_final (but not speech_final) - spawn turn detection in background
        if result.is_final {
            self.handle_turn_detection(result.clone(), speech_final_state, turn_detector);
        }

        // Always return the original result immediately - no awaits in critical path
        Some(result)
    }

    /// Fast synchronous check if result should be delivered
    /// Returns true if result should be processed and delivered to callback
    fn should_deliver_result(&self, result: &STTResult) -> bool {
        // Skip empty final results that aren't speech_final
        !(result.transcript.trim().is_empty() && result.is_final && !result.is_speech_final)
    }

    /// Handle turn detection logic asynchronously (non-blocking)
    /// This method spawns background tasks and doesn't block result delivery
    fn handle_turn_detection(
        &self,
        result: STTResult,
        speech_final_state: Arc<SyncRwLock<SpeechFinalState>>,
        turn_detector: Option<Arc<RwLock<TurnDetector>>>,
    ) {
        // Update text buffer, arm the segment, and capture the fire GENERATION —
        // all under ONE write lock so the snapshot the spawned tasks observe is
        // consistent (P0.3 / RC4). The tasks may only claim a fire while the
        // generation is unchanged; a fire/reset bumps it, so stale tasks are
        // structurally unable to double-fire or fire with another segment's text.
        let (buffered_text, is_new_segment, segment_generation) = {
            let mut state = speech_final_state.write();

            // CRITICAL: Cancel old task when new is_final arrives (person still talking)
            if let Some(old_handle) = state.turn_detection_handle.take() {
                debug!(
                    "New is_final arrived - cancelling previous turn detection (person still talking)"
                );
                old_handle.abort();
            }

            // O(1) amortized append; space-aware so multi-final segments don't
            // concatenate as "Hello there.How are you".
            append_with_space(&mut state.text_buffer, &result.transcript);

            // Bound the per-turn buffer (RC8): a pathological provider sending
            // endless is_finals must not grow memory without limit — keep the
            // most recent tail (what end-of-turn detection actually needs).
            const MAX_TURN_BUFFER_BYTES: usize = 32 * 1024;
            if state.text_buffer.len() > MAX_TURN_BUFFER_BYTES {
                let excess = state.text_buffer.len() - MAX_TURN_BUFFER_BYTES;
                let cut = state
                    .text_buffer
                    .char_indices()
                    .map(|(i, _)| i)
                    .find(|&i| i >= excess)
                    .unwrap_or(0);
                state.text_buffer.drain(..cut);
            }

            // Check if this is the first is_final for a new segment
            let is_new = state.segment_start_ms.load(Ordering::Acquire) == 0;

            state
                .waiting_for_speech_final
                .store(true, Ordering::Release);

            if is_new {
                let now_ms = self.get_current_time_ms();
                state.segment_start_ms.store(now_ms, Ordering::Release);
                let deadline_ms = now_ms + self.config.speech_final_hard_timeout_ms as usize;
                state
                    .hard_timeout_deadline_ms
                    .store(deadline_ms, Ordering::Release);
                debug!(
                    "Starting new speech segment - hard timeout will fire in {}ms at {}",
                    self.config.speech_final_hard_timeout_ms, deadline_ms
                );
                if let Some(old_handle) = state.hard_timeout_handle.take() {
                    debug!("Cancelling previous hard timeout task");
                    old_handle.abort();
                }
            }

            (
                state.text_buffer.clone(),
                is_new,
                state.fire_generation.load(Ordering::Relaxed),
            )
        };

        // Spawn tasks OUTSIDE the lock (they only ever act through the
        // generation-checked claim, so a racing real final between the lock
        // release and the handle store is harmless).
        let detection_handle = self.create_detection_task(
            result,
            buffered_text,
            speech_final_state.clone(),
            turn_detector,
            segment_generation,
        );

        let mut state = speech_final_state.write();
        state.turn_detection_handle = Some(detection_handle);
        if is_new_segment {
            let hard_timeout_handle =
                self.create_hard_timeout_task(speech_final_state.clone(), segment_generation);
            state.hard_timeout_handle = Some(hard_timeout_handle);
        }
    }

    /// Handle a real speech_final result
    fn handle_real_speech_final(
        &self,
        mut result: STTResult,
        speech_final_state: Arc<SyncRwLock<SpeechFinalState>>,
        now_ms: usize,
    ) -> Option<STTResult> {
        let mut state = speech_final_state.write();

        // Check for duplicate within the configured window (compare the RAW
        // provider fragment, before enrichment below).
        if self.is_duplicate_speech_final(&state, &result.transcript, now_ms) {
            debug!(
                "Ignoring duplicate real speech_final - turn detection fired {}ms ago",
                now_ms.saturating_sub(state.turn_detection_last_fired_ms.load(Ordering::Acquire))
            );
            return None;
        }

        // Cancel any pending detection tasks
        self.cancel_detection_task(&mut state);

        // SEGMENT TRANSCRIPT TRUTH: earlier is_final fragments of this segment
        // were accumulated in text_buffer; the provider's speech_final result
        // carries only the LAST fragment. Turn policy must see the FULL
        // segment ("Hello there." + "How are you") or multi-final utterances
        // run the LLM truncated. The full text rides `segment_transcript`;
        // `transcript` keeps the raw fragment so CLIENT EGRESS is unchanged
        // (clients that assemble finals by concatenation would otherwise see
        // every earlier fragment duplicated — review wf_5772cd64 #6).
        if !state.text_buffer.is_empty() {
            let mut full = std::mem::take(&mut state.text_buffer);
            append_with_space(&mut full, &result.transcript);
            result.segment_transcript = Some(full);
        }

        // Reset state for next speech segment
        self.reset_speech_state(&mut state);

        Some(result)
    }

    /// Create a hard timeout task that enforces the maximum wait time
    ///
    /// This task ensures that every speech segment gets a speech_final within
    /// speech_final_hard_timeout_ms (default 5 seconds), regardless of whether
    /// the STT provider sends speech_final or the turn detector confirms.
    ///
    /// # Arguments
    /// * `speech_final_state` - Shared state for speech final tracking
    /// * `segment_start_ms` - When the current speech segment started
    fn create_hard_timeout_task(
        &self,
        speech_final_state: Arc<SyncRwLock<SpeechFinalState>>,
        segment_generation: usize,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            // Deadline-rereading loop (P0.3/H5): the task sleeps TOWARD the
            // deadline atomic and re-validates it on every wake. A moved or
            // cleared deadline, a bumped generation, or a completed fire all
            // make this task exit without firing — an orphaned timer can never
            // force a stale speech_final.
            loop {
                let (deadline_ms, generation_now, waiting) = {
                    let state = speech_final_state.read();
                    (
                        state.hard_timeout_deadline_ms.load(Ordering::Acquire),
                        state.fire_generation.load(Ordering::Relaxed),
                        state.waiting_for_speech_final.load(Ordering::Acquire),
                    )
                };

                if deadline_ms == 0 || generation_now != segment_generation || !waiting {
                    debug!("Hard timeout cancelled - segment fired/reset/moved on");
                    return;
                }

                let now_ms = Self::get_current_time_ms_static();
                if now_ms >= deadline_ms {
                    tracing::warn!(
                        "Hard timeout fired - forcing speech_final (no real speech_final \
                         or turn detection confirmation received)"
                    );
                    Self::fire_forced_speech_final(
                        speech_final_state,
                        segment_generation,
                        "hard_timeout_fallback",
                        true,
                    )
                    .await;
                    return;
                }

                tokio::time::sleep(Duration::from_millis((deadline_ms - now_ms) as u64)).await;
            }
        })
    }

    /// Create a detection task that waits for STT provider, then uses turn detection as fallback
    ///
    /// Voice AI Best Practice Logic:
    /// 1. Wait for STT provider to send real speech_final (they see the audio stream)
    /// 2. If STT is silent and text hasn't changed, run turn detection to confirm
    /// 3. Only fire artificial speech_final if turn detection confirms turn is complete
    fn create_detection_task(
        &self,
        result: STTResult,
        buffered_text: String,
        speech_final_state: Arc<SyncRwLock<SpeechFinalState>>,
        turn_detector: Option<Arc<RwLock<TurnDetector>>>,
        segment_generation: usize,
    ) -> JoinHandle<()> {
        // A-G2: floor extended to the provider's TTFS p99 (slow providers'
        // real finals must not be beaten by a forced fire); a FINALIZED
        // result (provider acked: nothing more coming) collapses back to the
        // user-resume floor.
        let stt_wait_ms = self.config.effective_wait_ms(result.is_finalized);
        let inference_timeout_ms = self.config.turn_detection_inference_timeout_ms;

        tokio::spawn(async move {
            // PHASE 1: Wait for STT provider to send real speech_final
            // This is the primary path - we trust the STT provider first
            debug!(
                "Waiting {}ms for real speech_final from STT provider",
                stt_wait_ms
            );
            tokio::time::sleep(Duration::from_millis(stt_wait_ms)).await;

            // Check if we should still fire (not cancelled by real speech_final or new is_final)
            let should_continue = {
                let state = speech_final_state.read();
                state.waiting_for_speech_final.load(Ordering::Acquire)
            };

            if !should_continue {
                debug!("Turn detection cancelled - real speech_final arrived or new is_final");
                return;
            }

            // PHASE 2: STT didn't send speech_final - verify with turn detection
            let detection_method = if let Some(detector) = turn_detector {
                // Check if text buffer has changed (new transcripts arrived)
                let current_text = {
                    let state = speech_final_state.read();
                    state.text_buffer.clone()
                };

                // If text changed, someone is still talking - don't fire
                if current_text != buffered_text {
                    info!(
                        "Text buffer changed during wait (old: '{}', new: '{}') - person still talking, not firing",
                        buffered_text, current_text
                    );
                    return;
                }

                // Text hasn't changed - run turn detection to confirm turn is complete
                debug!(
                    "STT silent for {}ms, running turn detection to confirm",
                    stt_wait_ms
                );
                let turn_result =
                    tokio::time::timeout(Duration::from_millis(inference_timeout_ms), async {
                        let detector_guard = detector.read().await;
                        detector_guard.is_turn_complete(&current_text).await
                    })
                    .await;

                match turn_result {
                    Ok(Ok(true)) => {
                        info!(
                            "Turn detection confirms turn complete - firing artificial speech_final"
                        );
                        "turn_detection_confirmed"
                    }
                    Ok(Ok(false)) => {
                        info!(
                            "Turn detection says turn incomplete - not firing (person may still be thinking)"
                        );
                        return; // Don't fire - person may continue speaking
                    }
                    Ok(Err(e)) => {
                        tracing::warn!(
                            "Turn detection error: {:?} - firing as fallback ({}ms silence)",
                            e,
                            stt_wait_ms
                        );
                        "turn_detection_error_fallback"
                    }
                    Err(_) => {
                        tracing::warn!(
                            "Turn detection inference timeout after {}ms - firing as fallback",
                            inference_timeout_ms
                        );
                        "inference_timeout_fallback"
                    }
                }
            } else {
                // No turn detector - fire based on silence duration alone
                info!("No turn detector - firing after {}ms silence", stt_wait_ms);
                "no_detector_timeout"
            };

            // PHASE 3: Fire artificial speech_final (generation-checked claim)
            let _ = result; // original interim result not delivered again
            let _ = buffered_text; // claim takes the live buffer under the lock
            Self::fire_forced_speech_final(
                speech_final_state,
                segment_generation,
                detection_method,
                false,
            )
            .await;
        })
    }

    /// Fire a forced speech_final event via an ATOMIC CLAIM (P0.3 / RC4).
    ///
    /// Every check and every mutation happens under ONE write lock with no
    /// awaits inside: generation match + waiting flag → claim (bump generation,
    /// take buffer, drop timers, clear deadlines) → release → fire the callback
    /// from THIS task (the caller-fires-callback contract is unchanged). Two
    /// racing forced paths can never both claim; a real speech_final that beat
    /// us bumped the generation, so our claim refuses.
    async fn fire_forced_speech_final(
        speech_final_state: Arc<SyncRwLock<SpeechFinalState>>,
        segment_generation: usize,
        detection_method: &str,
        claimer_is_hard_timeout: bool,
    ) {
        let claimed = {
            let mut state = speech_final_state.write();
            let generation_ok =
                state.fire_generation.load(Ordering::Relaxed) == segment_generation;
            let still_waiting = state.waiting_for_speech_final.load(Ordering::Acquire);
            if !generation_ok || !still_waiting {
                debug!(
                    generation_ok,
                    still_waiting, "forced speech_final claim refused (lost the race)"
                );
                None
            } else {
                state.fire_generation.fetch_add(1, Ordering::Relaxed);
                let fire_time_ms = Self::get_current_time_ms_static();
                state
                    .turn_detection_last_fired_ms
                    .store(fire_time_ms, Ordering::Release);
                let text = std::mem::take(&mut state.text_buffer);
                // Keep a copy for the duplicate window (a late provider
                // speech_final carrying a fragment of this text is a dup).
                state.last_forced_text = text.clone();
                state
                    .waiting_for_speech_final
                    .store(false, Ordering::Release);
                state.turn_detection_handle = None;
                // CRITICAL (review wf_5772cd64): when the CLAIMER IS the
                // hard-timeout task, aborting this handle aborts OURSELVES —
                // the user callback chain below (egress → controller →
                // run_turn → LLM) would be killed at its first pending await
                // with the buffer already consumed (turn lost, no retry).
                // Mirror the detection-task discipline one line above: the
                // claimer's own handle is dropped WITHOUT abort; only the
                // OTHER task is aborted.
                if let Some(handle) = state.hard_timeout_handle.take()
                    && !claimer_is_hard_timeout
                {
                    handle.abort();
                }
                state.segment_start_ms.store(0, Ordering::Release);
                state.hard_timeout_deadline_ms.store(0, Ordering::Release);
                state.user_callback.clone().map(|cb| (cb, text))
            }
        };

        if let Some((callback, text)) = claimed {
            // The forced signal must CARRY the segment's buffered transcript:
            // both consumers (conversation orchestrator + DAG StreamDriver)
            // gate turns on `is_speech_final && !transcript.empty()`, so an
            // empty forced final never ran a turn — the entire timer /
            // turn-detector fallback path was dead (found during A-G0 contract
            // verification; the buffer was captured into last_forced_text and
            // only ever read by tests).
            // Egress keeps the legacy EMPTY transcript (clients assembling
            // finals by concatenation never see duplication); turn policy
            // reads the full segment from `segment_transcript`.
            let mut forced_result = STTResult::new(String::new(), true, true, 1.0);
            forced_result.segment_transcript = Some(text);
            info!("Forcing speech_final via {}", detection_method);
            callback(forced_result).await;
        }
    }

    /// Discard the current segment entirely (TurnEvent::ResetAggregation,
    /// A-G3): cancels any armed detection task and runs the GENERATION-
    /// BUMPING reset, so a sub-threshold segment (a cough below the
    /// MinWords gate) can neither fire its timers nor leak its text into
    /// the next real turn's buffer.
    pub fn reset_segment(&self, state: &mut SpeechFinalState) {
        self.cancel_detection_task(state);
        self.reset_speech_state(state);
    }

    /// Check if this is a duplicate speech_final event
    fn is_duplicate_speech_final(
        &self,
        state: &SpeechFinalState,
        transcript: &str,
        now_ms: usize,
    ) -> bool {
        let last_fired_ms = state.turn_detection_last_fired_ms.load(Ordering::Acquire);
        if last_fired_ms == 0
            || now_ms.saturating_sub(last_fired_ms) >= self.config.duplicate_window_ms
        {
            return false;
        }
        // Within the window: the forced fire delivered the FULL buffered
        // segment, while a late provider speech_final carries only its last
        // FRAGMENT. Compare NORMALIZED WORDS (case-folded, punctuation
        // stripped) because providers re-format late finals (smart_format:
        // "how are you" → "How are you?") — byte containment misses those
        // (review wf_5772cd64 #4). An empty fragment inside the window is
        // also a dup (a bare speech_final marker trailing the forced fire).
        // A late final carrying genuinely NEW words is NOT deduped — it
        // delivers (the user really said more).
        let late = normalize_words(transcript);
        if late.is_empty() {
            return true;
        }
        let forced: std::collections::HashSet<String> =
            normalize_words(&state.last_forced_text).into_iter().collect();
        late.iter().all(|w| forced.contains(w))
    }

    /// Cancel any existing detection task
    fn cancel_detection_task(&self, state: &mut SpeechFinalState) {
        if let Some(handle) = state.turn_detection_handle.take() {
            debug!("Cancelling pending turn detection task");
            handle.abort();
            state
                .waiting_for_speech_final
                .store(false, Ordering::Release);
        }

        // Also cancel hard timeout handle if present
        if let Some(handle) = state.hard_timeout_handle.take() {
            debug!("Cancelling pending hard timeout task");
            handle.abort();
        }
    }

    /// Reset speech state for next segment
    fn reset_speech_state(&self, state: &mut SpeechFinalState) {
        // Invalidate every in-flight timer/turn-detect claim for this segment
        // (P0.3): a task that observed the old generation can no longer fire.
        state.fire_generation.fetch_add(1, Ordering::Relaxed);
        state.text_buffer.clear();
        state.last_forced_text.clear();
        state
            .waiting_for_speech_final
            .store(false, Ordering::Release);
        state
            .turn_detection_last_fired_ms
            .store(0, Ordering::Release);
        state.segment_start_ms.store(0, Ordering::Release);
        state.hard_timeout_deadline_ms.store(0, Ordering::Release);

        // Cancel and clear hard timeout handle if present
        if let Some(handle) = state.hard_timeout_handle.take() {
            handle.abort();
        }
    }

    /// Get current time in milliseconds
    fn get_current_time_ms(&self) -> usize {
        Self::get_current_time_ms_static()
    }

    /// Static helper to get current time in milliseconds.
    ///
    /// MONOTONIC, process-relative (P0.4 / RC3): all speech-final deadlines and
    /// dedup windows are relative intervals; wall-clock here broke them under
    /// NTP steps / VM restore (epoch-0 on `duration_since` failure flooded
    /// hard-timeout logs and mis-armed deadlines).
    /// Clamped to ≥1: `0` is the RESERVED sentinel for "unset" in
    /// `segment_start_ms` / `turn_detection_last_fired_ms` /
    /// `hard_timeout_deadline_ms`. The monotonic clock is process-relative,
    /// so a segment armed in the process's first millisecond would store 0 —
    /// making the NEXT is_final read it as "no active segment" and re-arm
    /// (restart!) the hard-timeout backstop, deferring the forced fire
    /// indefinitely while fragments keep arriving.
    fn get_current_time_ms_static() -> usize {
        clamp_clock(super::state::now_monotonic_ms())
    }
}

/// Default processor instance with standard configuration
impl Default for STTResultProcessor {
    fn default() -> Self {
        Self::new(STTProcessingConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::voice_manager::callbacks::STTCallback;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    // --- A-G2: TTFS-aware effective wait ---

    #[test]
    fn effective_wait_unchanged_without_ttfs() {
        let c = STTProcessingConfig::new(600, 100, 2500, 500);
        assert_eq!(c.effective_wait_ms(false), 600);
        assert_eq!(c.effective_wait_ms(true), 600);
    }

    #[test]
    fn slow_provider_extends_wait_fast_keeps_floor() {
        // Slow provider (ttfs 900 > floor 600): wait extends so the real
        // final isn't beaten by a forced fire.
        let slow = STTProcessingConfig::new(600, 100, 2500, 500).with_stt_ttfs_p99_ms(Some(900));
        assert_eq!(slow.effective_wait_ms(false), 900);
        // Fast provider (ttfs 350 < floor): the floor is a USER-resume
        // window — never shortened by STT speed.
        let fast = STTProcessingConfig::new(600, 100, 2500, 500).with_stt_ttfs_p99_ms(Some(350));
        assert_eq!(fast.effective_wait_ms(false), 600);
    }

    #[test]
    fn finalized_collapses_extension_to_floor() {
        // The provider acked finalize (nothing more coming): no reason to
        // wait out the slow-provider extension; the floor remains.
        let c = STTProcessingConfig::new(600, 100, 2500, 500).with_stt_ttfs_p99_ms(Some(1200));
        assert_eq!(c.effective_wait_ms(false), 1200);
        assert_eq!(c.effective_wait_ms(true), 600);
    }

    #[test]
    fn wait_never_crowds_out_hard_timeout() {
        // ttfs larger than the hard timeout: clamped to 2/3 of it so the
        // backstop still acts.
        let c = STTProcessingConfig::new(600, 100, 1500, 500).with_stt_ttfs_p99_ms(Some(5000));
        assert_eq!(c.effective_wait_ms(false), 1000);
    }

    #[test]
    fn clock_never_collides_with_unset_sentinel() {
        // 0 is the reserved "unset" sentinel for segment_start_ms /
        // last_fired_ms / hard_timeout_deadline_ms. The process-relative
        // monotonic clock RETURNS 0 in the process's first millisecond — a
        // segment armed then would read as "no active segment" on the next
        // is_final, silently RESTARTING the hard-timeout backstop. The pin
        // is on the PURE clamp (a warm process can't reproduce raw 0).
        assert_eq!(clamp_clock(0), 1, "first-millisecond clock must not store the sentinel");
        assert_eq!(clamp_clock(1), 1);
        assert_eq!(clamp_clock(123_456), 123_456);
        assert!(STTResultProcessor::get_current_time_ms_static() >= 1);
    }

    #[test]
    fn slow_ttfs_below_large_floor_keeps_floor() {
        // The (ttfs < floor && floor > 2/3*hard) cell of the wait matrix
        // (review wf_85659e16): the extension branch must not even engage —
        // the floor passes through verbatim.
        let c = STTProcessingConfig::new(2000, 100, 2500, 500).with_stt_ttfs_p99_ms(Some(900));
        assert_eq!(c.effective_wait_ms(false), 2000);
        assert_eq!(c.effective_wait_ms(true), 2000);
    }

    #[test]
    fn clamp_never_shrinks_configured_floor() {
        // Operator set floor > ⅔×hard-timeout (their prerogative — the
        // pre-A-G2 semantics honored it verbatim). The clamp applies to the
        // TTFS EXTENSION only; it must never cut the explicit floor (review
        // wf_5772cd64 #11).
        let no_ttfs = STTProcessingConfig::new(2000, 100, 2500, 500);
        assert_eq!(no_ttfs.effective_wait_ms(false), 2000);
        let with_ttfs =
            STTProcessingConfig::new(2000, 100, 2500, 500).with_stt_ttfs_p99_ms(Some(9000));
        assert_eq!(
            with_ttfs.effective_wait_ms(false),
            2000,
            "extension clamp bottoms out at the floor, never below it"
        );
    }

    /// Behavioral: a finalized final shortens the live detection wait — the
    /// forced fire happens at the floor, not the TTFS-extended wait.
    #[tokio::test]
    async fn finalized_shortens_live_detection_wait() {
        let config = STTProcessingConfig::new(40, 30, 5000, 100)
            .with_stt_ttfs_p99_ms(Some(10_000)); // extension would exceed the test budget
        let processor = STTResultProcessor::new(config);
        let fires = Arc::new(AtomicUsize::new(0));
        let fires_cb = fires.clone();
        let callback: STTCallback = Arc::new(move |result: STTResult| {
            let fires = fires_cb.clone();
            Box::pin(async move {
                if result.is_speech_final {
                    fires.fetch_add(1, Ordering::SeqCst);
                }
            }) as Pin<Box<dyn Future<Output = ()> + Send>>
        });
        let state = fresh_state(callback);

        // FINALIZED final: the wait collapses to the 40ms floor (without the
        // finalized flag it would be clamped to 2/3 × 5000 ≈ 3333ms — far
        // beyond this test's window).
        let finalized = STTResult::new("done now".into(), true, false, 0.9).finalized();
        let _ = processor.process_result(finalized, state.clone(), None).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            fires.load(Ordering::SeqCst),
            1,
            "finalized result must collapse the TTFS extension to the floor"
        );
    }

    fn fresh_state(callback: STTCallback) -> Arc<SyncRwLock<SpeechFinalState>> {
        Arc::new(SyncRwLock::new(SpeechFinalState {
            text_buffer: String::new(),
            turn_detection_handle: None,
            hard_timeout_handle: None,
            waiting_for_speech_final: AtomicBool::new(false),
            user_callback: Some(callback),
            turn_detection_last_fired_ms: AtomicUsize::new(0),
            last_forced_text: String::new(),
            segment_start_ms: AtomicUsize::new(0),
            hard_timeout_deadline_ms: AtomicUsize::new(0),
            fire_generation: AtomicUsize::new(0),
        }))
    }

    /// SEGMENT TRANSCRIPT TRUTH (pre-A-G0 contract fix): the forced/timer
    /// speech_final must CARRY the buffered segment text — both consumers gate
    /// turns on non-empty transcripts, so an empty forced final means the
    /// whole timer fallback never runs a turn (the pre-fix behavior).
    #[tokio::test]
    async fn forced_fire_carries_buffered_transcript() {
        let config = STTProcessingConfig {
            stt_speech_final_wait_ms: 30,
            turn_detection_inference_timeout_ms: 30,
            speech_final_hard_timeout_ms: 120,
            duplicate_window_ms: 100,
            stt_ttfs_p99_ms: None,
        };
        let processor = STTResultProcessor::new(config);

        // Capture (turn_transcript, raw transcript): policy must get the full
        // segment while the raw fragment stays what clients always saw.
        let forced_texts: Arc<parking_lot::Mutex<Vec<(String, String)>>> =
            Arc::new(parking_lot::Mutex::new(Vec::new()));
        let sink = forced_texts.clone();
        let callback: STTCallback = Arc::new(move |result: STTResult| {
            let sink = sink.clone();
            Box::pin(async move {
                if result.is_speech_final {
                    sink.lock()
                        .push((result.turn_transcript().to_string(), result.transcript));
                }
            }) as Pin<Box<dyn Future<Output = ()> + Send>>
        });
        let state = fresh_state(callback);

        // Two finals, never a provider speech_final → the timer path fires.
        let _ = processor
            .process_result(
                STTResult::new("Hello there.".into(), true, false, 0.9),
                state.clone(),
                None,
            )
            .await;
        let _ = processor
            .process_result(
                STTResult::new("How are you".into(), true, false, 0.9),
                state.clone(),
                None,
            )
            .await;

        tokio::time::sleep(Duration::from_millis(250)).await;

        let texts = forced_texts.lock();
        assert_eq!(texts.len(), 1, "exactly one forced speech_final");
        assert_eq!(
            texts[0].0, "Hello there. How are you",
            "forced final must carry the FULL space-joined segment text for turn policy"
        );
        assert!(
            texts[0].1.is_empty(),
            "the forced final's RAW transcript stays empty — client egress already \
             received every fragment; re-sending the joined text would duplicate them"
        );
    }

    /// A real provider speech_final delivers the full segment (buffered
    /// fragments + its own), not just the last fragment — otherwise long
    /// utterances run the LLM with truncated input.
    #[tokio::test]
    async fn real_speech_final_includes_prior_final_fragments() {
        let config = STTProcessingConfig {
            stt_speech_final_wait_ms: 500,
            turn_detection_inference_timeout_ms: 100,
            speech_final_hard_timeout_ms: 5000,
            duplicate_window_ms: 100,
            stt_ttfs_p99_ms: None,
        };
        let processor = STTResultProcessor::new(config);
        let noop: STTCallback = Arc::new(|_| Box::pin(async {}));
        let state = fresh_state(noop);

        let _ = processor
            .process_result(
                STTResult::new("Hello there.".into(), true, false, 0.9),
                state.clone(),
                None,
            )
            .await;
        let delivered = processor
            .process_result(
                STTResult::new("How are you".into(), true, true, 0.9),
                state.clone(),
                None,
            )
            .await
            .expect("real speech_final must be delivered");
        // Turn policy sees the full segment; the raw fragment is untouched so
        // client egress (which already saw "Hello there.") gets no duplicate.
        assert_eq!(delivered.turn_transcript(), "Hello there. How are you");
        assert_eq!(delivered.transcript, "How are you");
        assert!(delivered.is_speech_final);
        // Buffer consumed: the next segment starts clean.
        assert!(state.read().text_buffer.is_empty());
    }

    /// BOTH forced-fire claimers (detection task AND hard-timeout task)
    /// must run the user callback to COMPLETION through a pending await —
    /// whichever wins under any future wait-clamp semantics (review
    /// wf_85659e16: the original pin only held while the hard-timeout task
    /// happened to be the claimer).
    #[tokio::test]
    async fn detection_claimer_survives_pending_await_in_callback() {
        let config = STTProcessingConfig {
            // Detection wait SHORTER than the hard timeout: the detection
            // task is the claimer.
            stt_speech_final_wait_ms: 40,
            turn_detection_inference_timeout_ms: 30,
            speech_final_hard_timeout_ms: 5_000,
            duplicate_window_ms: 100,
            stt_ttfs_p99_ms: None,
        };
        let processor = STTResultProcessor::new(config);
        let completed = Arc::new(AtomicUsize::new(0));
        let completed_cb = completed.clone();
        let callback: STTCallback = Arc::new(move |result: STTResult| {
            let completed = completed_cb.clone();
            Box::pin(async move {
                if result.is_speech_final {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    completed.fetch_add(1, Ordering::SeqCst);
                }
            }) as Pin<Box<dyn Future<Output = ()> + Send>>
        });
        let state = fresh_state(callback);
        let _ = processor
            .process_result(
                STTResult::new("don't kill me either".into(), true, false, 0.9),
                state.clone(),
                None,
            )
            .await;
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(
            completed.load(Ordering::SeqCst),
            1,
            "detection-claimer fire must run the callback to completion"
        );
    }

    /// CRITICAL (review wf_5772cd64 #1): when the HARD-TIMEOUT task itself
    /// claims the fire, it must not abort its own JoinHandle — pre-fix it
    /// did, killing itself at the next await INSIDE the user callback, so
    /// any callback with a real pending await never completed (no turn ran).
    #[tokio::test]
    async fn hard_timeout_fire_survives_pending_await_in_callback() {
        let config = STTProcessingConfig {
            // Detection wait LONGER than the hard timeout: the hard-timeout
            // task is the claimer.
            stt_speech_final_wait_ms: 5_000,
            turn_detection_inference_timeout_ms: 50,
            speech_final_hard_timeout_ms: 100,
            duplicate_window_ms: 100,
            stt_ttfs_p99_ms: None,
        };
        let processor = STTResultProcessor::new(config);
        let completed = Arc::new(AtomicUsize::new(0));
        let completed_cb = completed.clone();
        let callback: STTCallback = Arc::new(move |result: STTResult| {
            let completed = completed_cb.clone();
            Box::pin(async move {
                if result.is_speech_final {
                    // The pending await the self-abort used to die on.
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    completed.fetch_add(1, Ordering::SeqCst);
                }
            }) as Pin<Box<dyn Future<Output = ()> + Send>>
        });
        let state = fresh_state(callback);

        let _ = processor
            .process_result(
                STTResult::new("don't kill me".into(), true, false, 0.9),
                state.clone(),
                None,
            )
            .await;
        // 100ms hard timeout + 50ms callback await + slack.
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(
            completed.load(Ordering::SeqCst),
            1,
            "hard-timeout fire must run the callback to completion (no self-abort)"
        );
    }

    /// Dedup must survive provider FORMATTING differences (review wf_5772cd64
    /// #4): the late real speech_final often arrives punctuated/capitalized
    /// while the forced text was raw — exact substring matching missed it and
    /// ran a second turn.
    #[tokio::test]
    async fn forced_dedup_survives_formatting_differences() {
        let config = STTProcessingConfig {
            stt_speech_final_wait_ms: 30,
            turn_detection_inference_timeout_ms: 30,
            speech_final_hard_timeout_ms: 120,
            duplicate_window_ms: 5_000,
            stt_ttfs_p99_ms: None,
        };
        let processor = STTResultProcessor::new(config);
        let noop: STTCallback = Arc::new(|_| Box::pin(async {}));
        let state = fresh_state(noop);

        let _ = processor
            .process_result(
                STTResult::new("book a flight to paris".into(), true, false, 0.9),
                state.clone(),
                None,
            )
            .await;
        tokio::time::sleep(Duration::from_millis(250)).await;

        // Punctuated + capitalized fragment of the just-forced text.
        let dup = processor
            .process_result(
                STTResult::new("To Paris.".into(), true, true, 0.9),
                state.clone(),
                None,
            )
            .await;
        assert!(dup.is_none(), "formatting-only differences are still duplicates");

        // But genuinely NEW words within the window are NOT a duplicate.
        let fresh = processor
            .process_result(
                STTResult::new("and a hotel too".into(), true, true, 0.9),
                state.clone(),
                None,
            )
            .await;
        assert!(fresh.is_some(), "new content within the window must pass");
    }

    /// A late provider speech_final carrying a FRAGMENT of the just-forced
    /// text is a duplicate (containment, not equality) — no double turn.
    #[tokio::test]
    async fn late_fragment_after_forced_fire_is_deduplicated() {
        let config = STTProcessingConfig {
            stt_speech_final_wait_ms: 30,
            turn_detection_inference_timeout_ms: 30,
            speech_final_hard_timeout_ms: 120,
            duplicate_window_ms: 5_000,
            stt_ttfs_p99_ms: None,
        };
        let processor = STTResultProcessor::new(config);
        let fires = Arc::new(AtomicUsize::new(0));
        let fires_cb = fires.clone();
        let callback: STTCallback = Arc::new(move |result: STTResult| {
            let fires = fires_cb.clone();
            Box::pin(async move {
                if result.is_speech_final {
                    fires.fetch_add(1, Ordering::SeqCst);
                }
            }) as Pin<Box<dyn Future<Output = ()> + Send>>
        });
        let state = fresh_state(callback);

        let _ = processor
            .process_result(
                STTResult::new("book a flight to Paris".into(), true, false, 0.9),
                state.clone(),
                None,
            )
            .await;
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(fires.load(Ordering::SeqCst), 1, "forced fire happened");

        // The provider's own (late) speech_final with the trailing fragment.
        let dup = processor
            .process_result(
                STTResult::new("to Paris".into(), true, true, 0.9),
                state.clone(),
                None,
            )
            .await;
        assert!(dup.is_none(), "fragment of the forced text within the window is a dup");
    }

    #[test]
    fn append_with_space_joins_cleanly() {
        let mut b = String::new();
        append_with_space(&mut b, "Hello there.");
        append_with_space(&mut b, "How are you");
        assert_eq!(b, "Hello there. How are you");
        let mut b2 = "ends with space ".to_string();
        append_with_space(&mut b2, "next");
        assert_eq!(b2, "ends with space next");
        let mut b3 = "x".to_string();
        append_with_space(&mut b3, "");
        assert_eq!(b3, "x");
    }

    #[tokio::test]
    async fn test_hard_timeout_fires_when_no_speech_final() {
        // Test that hard timeout fires after configured duration when no speech_final arrives
        let config = STTProcessingConfig {
            stt_speech_final_wait_ms: 50,
            turn_detection_inference_timeout_ms: 50,
            speech_final_hard_timeout_ms: 200, // 200ms hard timeout
            duplicate_window_ms: 100,
            stt_ttfs_p99_ms: None,
        };

        let processor = STTResultProcessor::new(config);

        // Track callback invocations
        let callback_count = Arc::new(AtomicUsize::new(0));
        let callback_count_clone = callback_count.clone();

        let callback: STTCallback = Arc::new(move |result: STTResult| {
            let count = callback_count_clone.clone();
            Box::pin(async move {
                if result.is_speech_final {
                    count.fetch_add(1, Ordering::SeqCst);
                }
            }) as Pin<Box<dyn Future<Output = ()> + Send>>
        });

        let state = Arc::new(SyncRwLock::new(SpeechFinalState {
            text_buffer: String::new(),
            turn_detection_handle: None,
            hard_timeout_handle: None,
            waiting_for_speech_final: AtomicBool::new(false),
            user_callback: Some(callback),
            turn_detection_last_fired_ms: AtomicUsize::new(0),
            last_forced_text: String::new(),
            segment_start_ms: AtomicUsize::new(0),
            hard_timeout_deadline_ms: AtomicUsize::new(0),
            fire_generation: AtomicUsize::new(0),
        }));

        // Send an is_final result (no speech_final)
        let result = STTResult::new("Hello world".to_string(), true, false, 0.95);

        // Process the result - should trigger turn detection and hard timeout
        let processed = processor.process_result(result, state.clone(), None).await;
        assert!(processed.is_some());

        // Wait for hard timeout to fire (200ms + buffer)
        tokio::time::sleep(Duration::from_millis(300)).await;

        // Hard timeout should have fired the callback
        assert_eq!(
            callback_count.load(Ordering::SeqCst),
            1,
            "Hard timeout should have fired speech_final callback"
        );

        // State should be reset
        let final_state = state.read();
        assert!(!final_state.waiting_for_speech_final.load(Ordering::Acquire));
        assert_eq!(final_state.segment_start_ms.load(Ordering::Acquire), 0);
        assert_eq!(
            final_state.hard_timeout_deadline_ms.load(Ordering::Acquire),
            0
        );
    }

    #[tokio::test]
    async fn test_hard_timeout_cancelled_by_real_speech_final() {
        // Test that hard timeout is cancelled when real speech_final arrives
        let config = STTProcessingConfig {
            stt_speech_final_wait_ms: 50,
            turn_detection_inference_timeout_ms: 50,
            speech_final_hard_timeout_ms: 500, // Long timeout
            duplicate_window_ms: 100,
            stt_ttfs_p99_ms: None,
        };

        let processor = STTResultProcessor::new(config);

        let callback_count = Arc::new(AtomicUsize::new(0));
        let callback_count_clone = callback_count.clone();

        let callback: STTCallback = Arc::new(move |result: STTResult| {
            let count = callback_count_clone.clone();
            Box::pin(async move {
                if result.is_speech_final {
                    count.fetch_add(1, Ordering::SeqCst);
                }
            }) as Pin<Box<dyn Future<Output = ()> + Send>>
        });

        let state = Arc::new(SyncRwLock::new(SpeechFinalState {
            text_buffer: String::new(),
            turn_detection_handle: None,
            hard_timeout_handle: None,
            waiting_for_speech_final: AtomicBool::new(false),
            user_callback: Some(callback),
            turn_detection_last_fired_ms: AtomicUsize::new(0),
            last_forced_text: String::new(),
            segment_start_ms: AtomicUsize::new(0),
            hard_timeout_deadline_ms: AtomicUsize::new(0),
            fire_generation: AtomicUsize::new(0),
        }));

        // Send an is_final result
        let is_final_result = STTResult::new("Hello".to_string(), true, false, 0.95);

        processor
            .process_result(is_final_result, state.clone(), None)
            .await;

        // Wait a bit
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Send real speech_final before hard timeout fires
        let speech_final_result = STTResult::new("Hello world".to_string(), true, true, 0.95);

        processor
            .process_result(speech_final_result, state.clone(), None)
            .await;

        // Wait to ensure hard timeout doesn't fire
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Should only have 1 callback (from real speech_final, not hard timeout)
        assert_eq!(
            callback_count.load(Ordering::SeqCst),
            1,
            "Only real speech_final should fire, not hard timeout"
        );
    }

    #[tokio::test]
    async fn test_hard_timeout_not_restarted_by_new_is_final() {
        // Test that hard timeout continues from first is_final when new is_final arrives
        let config = STTProcessingConfig {
            stt_speech_final_wait_ms: 300, // Long turn detection wait
            turn_detection_inference_timeout_ms: 50,
            speech_final_hard_timeout_ms: 200, // Hard timeout fires first
            duplicate_window_ms: 100,
            stt_ttfs_p99_ms: None,
        };

        let processor = STTResultProcessor::new(config);

        let callback_count = Arc::new(AtomicUsize::new(0));
        let callback_count_clone = callback_count.clone();

        let callback: STTCallback = Arc::new(move |result: STTResult| {
            let count = callback_count_clone.clone();
            Box::pin(async move {
                if result.is_speech_final {
                    count.fetch_add(1, Ordering::SeqCst);
                }
            }) as Pin<Box<dyn Future<Output = ()> + Send>>
        });

        let state = Arc::new(SyncRwLock::new(SpeechFinalState {
            text_buffer: String::new(),
            turn_detection_handle: None,
            hard_timeout_handle: None,
            waiting_for_speech_final: AtomicBool::new(false),
            user_callback: Some(callback),
            turn_detection_last_fired_ms: AtomicUsize::new(0),
            last_forced_text: String::new(),
            segment_start_ms: AtomicUsize::new(0),
            hard_timeout_deadline_ms: AtomicUsize::new(0),
            fire_generation: AtomicUsize::new(0),
        }));

        // Send first is_final at t=0
        let result1 = STTResult::new("Hello".to_string(), true, false, 0.95);

        processor.process_result(result1, state.clone(), None).await;

        // Wait a bit (but less than hard timeout)
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Send another is_final at t=100ms (person still talking)
        let result2 = STTResult::new(" world".to_string(), true, false, 0.95);

        processor.process_result(result2, state.clone(), None).await;

        // Wait for hard timeout (should fire at t=200ms from first is_final)
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Hard timeout should fire once at t=200ms (not restarted by second is_final)
        assert_eq!(
            callback_count.load(Ordering::SeqCst),
            1,
            "Hard timeout should fire once based on first is_final timestamp"
        );
    }

    #[tokio::test]
    async fn test_segment_timing_reset_after_speech_final() {
        // Test that segment timing is properly reset after speech_final
        let config = STTProcessingConfig::default();
        let processor = STTResultProcessor::new(config);

        let callback_count = Arc::new(AtomicUsize::new(0));
        let callback_count_clone = callback_count.clone();

        let callback: STTCallback = Arc::new(move |result: STTResult| {
            let count = callback_count_clone.clone();
            Box::pin(async move {
                if result.is_speech_final {
                    count.fetch_add(1, Ordering::SeqCst);
                }
            }) as Pin<Box<dyn Future<Output = ()> + Send>>
        });

        let state = Arc::new(SyncRwLock::new(SpeechFinalState {
            text_buffer: String::new(),
            turn_detection_handle: None,
            hard_timeout_handle: None,
            waiting_for_speech_final: AtomicBool::new(false),
            user_callback: Some(callback),
            turn_detection_last_fired_ms: AtomicUsize::new(0),
            last_forced_text: String::new(),
            segment_start_ms: AtomicUsize::new(0),
            hard_timeout_deadline_ms: AtomicUsize::new(0),
            fire_generation: AtomicUsize::new(0),
        }));

        // Send is_final
        let result = STTResult::new("First utterance".to_string(), true, false, 0.95);

        processor.process_result(result, state.clone(), None).await;

        // Verify segment timing was set
        {
            let s = state.read();
            assert_ne!(s.segment_start_ms.load(Ordering::Acquire), 0);
            assert_ne!(s.hard_timeout_deadline_ms.load(Ordering::Acquire), 0);
        }

        // Send real speech_final
        let speech_final = STTResult::new("First utterance complete".to_string(), true, true, 0.95);

        processor
            .process_result(speech_final, state.clone(), None)
            .await;

        // Verify segment timing was reset
        {
            let s = state.read();
            assert_eq!(s.segment_start_ms.load(Ordering::Acquire), 0);
            assert_eq!(s.hard_timeout_deadline_ms.load(Ordering::Acquire), 0);
        }

        // Send new is_final for next utterance
        let result2 = STTResult::new("Second utterance".to_string(), true, false, 0.95);

        processor.process_result(result2, state.clone(), None).await;

        // Verify segment timing was set again
        {
            let s = state.read();
            assert_ne!(s.segment_start_ms.load(Ordering::Acquire), 0);
            assert_ne!(s.hard_timeout_deadline_ms.load(Ordering::Acquire), 0);
        }
    }

    /// Build an armed state (waiting=true) with a counting speech_final callback.
    fn armed_state_with_counter() -> (Arc<SyncRwLock<SpeechFinalState>>, Arc<AtomicUsize>) {
        let callback_count = Arc::new(AtomicUsize::new(0));
        let callback_count_clone = callback_count.clone();
        let callback: STTCallback = Arc::new(move |result: STTResult| {
            let count = callback_count_clone.clone();
            Box::pin(async move {
                if result.is_speech_final {
                    count.fetch_add(1, Ordering::SeqCst);
                }
            }) as Pin<Box<dyn Future<Output = ()> + Send>>
        });
        let state = Arc::new(SyncRwLock::new(SpeechFinalState {
            text_buffer: "hello".to_string(),
            turn_detection_handle: None,
            hard_timeout_handle: None,
            waiting_for_speech_final: AtomicBool::new(true),
            user_callback: Some(callback),
            turn_detection_last_fired_ms: AtomicUsize::new(0),
            last_forced_text: String::new(),
            segment_start_ms: AtomicUsize::new(1),
            hard_timeout_deadline_ms: AtomicUsize::new(1),
            fire_generation: AtomicUsize::new(7),
        }));
        (state, callback_count)
    }

    /// P0.3 (C4): N racing forced fires for the SAME generation — the claim is
    /// a single-lock atomic transition, so exactly one wins.
    #[tokio::test]
    async fn racing_forced_fires_claim_exactly_once() {
        let (state, count) = armed_state_with_counter();
        let mut handles = Vec::new();
        for _ in 0..16 {
            let st = state.clone();
            handles.push(tokio::spawn(async move {
                STTResultProcessor::fire_forced_speech_final(st, 7, "race-test", false).await;
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "exactly ONE of the racing forced fires may claim"
        );
        assert_eq!(state.read().fire_generation.load(Ordering::Relaxed), 8);
    }

    /// P0.3 (H5): a task that observed an OLDER generation (its segment was
    /// already closed by a real speech_final) must refuse to fire.
    #[tokio::test]
    async fn stale_generation_cannot_fire() {
        let (state, count) = armed_state_with_counter();
        // A real speech_final closed the segment: generation bumped.
        state.read().fire_generation.fetch_add(1, Ordering::Relaxed);
        STTResultProcessor::fire_forced_speech_final(state.clone(), 7, "stale-timer", false).await;
        assert_eq!(
            count.load(Ordering::SeqCst),
            0,
            "a stale-generation timer must never force a speech_final"
        );
    }

    /// P0.3/P0.4 (H5): the hard-timeout task re-reads the deadline on wake —
    /// a cleared deadline (segment closed) means no fire, even though the task
    /// was already sleeping toward it.
    #[tokio::test]
    async fn cleared_deadline_disarms_inflight_hard_timeout() {
        let (state, count) = armed_state_with_counter();
        let now = super::super::state::now_monotonic_ms();
        state
            .read()
            .hard_timeout_deadline_ms
            .store(now + 150, Ordering::Release);

        let processor = STTResultProcessor::new(STTProcessingConfig {
            stt_speech_final_wait_ms: 5_000,
            turn_detection_inference_timeout_ms: 50,
            speech_final_hard_timeout_ms: 150,
            duplicate_window_ms: 100,
            stt_ttfs_p99_ms: None,
        });
        let handle = processor.create_hard_timeout_task(state.clone(), 7);

        // Segment closes (real final): deadline cleared + generation bumped,
        // but the timer task is ALREADY sleeping toward the old deadline.
        tokio::time::sleep(Duration::from_millis(30)).await;
        {
            let s = state.read();
            s.hard_timeout_deadline_ms.store(0, Ordering::Release);
            s.fire_generation.fetch_add(1, Ordering::Relaxed);
            s.waiting_for_speech_final.store(false, Ordering::Release);
        }

        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(handle.is_finished(), "timer task must exit once disarmed");
        assert_eq!(
            count.load(Ordering::SeqCst),
            0,
            "a disarmed hard timeout must never fire"
        );
    }

    /// P0.4 (C5): timing decisions use the monotonic process clock — two reads
    /// straddling a sleep are ordered and proportional, by construction
    /// independent of wall-clock (which can step backwards under NTP).
    #[tokio::test]
    async fn monotonic_clock_orders_and_measures() {
        let t0 = super::super::state::now_monotonic_ms();
        tokio::time::sleep(Duration::from_millis(25)).await;
        let t1 = super::super::state::now_monotonic_ms();
        assert!(t1 > t0, "monotonic clock must advance");
        assert!(t1 - t0 >= 20, "interval must reflect elapsed time");
    }
}

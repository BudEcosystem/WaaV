# Chapter 10. When services fail: pre-made lines in the session's voice

Added 2026-10-16 at the product owner's request. It extends chapter 5 (the clip bank) and chapter 9 (no rule-based
fallback for understanding the caller).

## What this part is and why it exists

A call depends on several services: the recogniser (speech-to-text), the fast front model, the orchestrator and the
background LLM, the agents, and the voice (text-to-speech). Any of them can fail or stall. Today a failure means
silence, a cut call, or an error event only the client sees. A person on the phone whose computer froze would say
"sorry, my system is acting up, give me a few seconds", and if it stays down, "I'm so sorry, my systems went down, I
have to go now; I'll get back to you as soon as they're back".

This chapter gives the agent those lines. They are **recorded in advance in the session's own voice**, because the
service that fails may be the voice itself, or the model that would have written the words. They are not a fallback for
understanding the caller (chapter 9 has none): they only say, honestly, that something is wrong.

## How it works

### The lines

Each language pack gains a `failure` group of short lines, in the persona's style, in three kinds:

| Kind | When | Examples |
|---|---|---|
| `stall` | a service is slow or failing and may come back | "Sorry, my systems are acting up, give me a few seconds." / "Bear with me a moment, something's running slow on my side." / "Still with me? I'm nearly there." |
| `hearing` | the recogniser fails or stops returning results while the caller speaks | "Sorry, I'm having trouble hearing you right now; give me a second." |
| `exit` | the outage lasts beyond the operator's limit | "I'm so sorry, my systems went down and I have to go now. I'll get back to you as soon as they're back." / without a follow-up channel: "...please call back in a few minutes." |
| `back` | the service recovered after a `stall` | "Thanks for waiting, I'm back." |

**Honesty:** an `exit` line that promises to get back is used only when the session has a way to do it (a post-call
push address, the operator's follow-up configured, or a caller memory the operator's system acts on); otherwise the
variant without the promise is used. Lines never name the failing vendor.

### Recorded before they are needed

- **At session start**, with the clip bank of chapter 5, the gateway renders the session's `failure` lines in the
  session's voice and language and checks them like clips (not silent, length bound). Rendered lines are cached on disk
  by provider, voice, language and text (the same cache layout as chapter 5's `duplex.clips_dir`), so most sessions
  start with them already present.
- **Operator recordings** in `duplex.clips_dir/<provider>/<voice>/<language>/failure/<line id>.wav` replace rendered
  ones.
- **If the voice is down at session start** and nothing is cached for it, the server's default set is used: rendered
  once at server start with the server's default voice (or the operator's recordings), so every session has at least
  one voice's lines. Interaction mode is not refused for this; `ready` reports which set is loaded.
- The lines are played by the playback pump directly, like listening sounds, so they need no text-to-speech, no model
  and no network at the moment they play.

### When they play

A small **health watcher** in the session sees each dependency's errors and stalls from the code paths that already
report them (the recogniser session's errors and silence while the caller speaks, the front supervisor of chapter 2,
the background run and agent drivers' errors and timeouts with their breakers, and the speech side's synthesis
failures and watchdog). It is driven by health signals, never by what the caller said.

1. **Stall.** When a dependency the current turn needs has failed or exceeded its deadline (front: the supervisor's
   health check; orchestrator/LLM: no first token within `failure.stall_after_ms`, default 2,500 ms; synthesis: the
   floor's 10 s watchdog lowered to the same value; recogniser: errors or no result for 3 s of caller speech), a
   `stall` line plays at the next slot (never over the caller). Further stall lines at most every 8 s, at most 3,
   rotated so none repeats.
2. **Back.** If the service recovers, a `back` line plays only if a `stall` was heard, and the turn continues
   (the pending answer is spoken, the task resumes).
3. **Exit.** If the outage lasts `failure.max_wait_s` (default 30 s) after the first stall, or a dependency the whole
   session needs is gone (the recogniser cannot reconnect; the front is not back within its restart window), the
   `exit` line plays, live tasks follow chapter 3's `tasks.session_end` rules, a `session_failed` event and the signed
   post-call push carry what was pending and why, and the session ends (or transfers to a person where the operator
   configured `human`, chapter 9).
4. Every line heard is recorded in the history (chapter 9) as a note, and in the client events
   (`interaction_updated` with `degraded` and the `failure` line id).

### What each failure does

| Failing service | Stall line | Exit when |
|---|---|---|
| Recogniser (speech-to-text) | `hearing` | it cannot reconnect within `max_wait_s` |
| Front model | `stall` | not back within the supervisor's restart window (then interaction mode ends, chapter 9) |
| Orchestrator / background LLM | `stall` while the caller waits for an answer | `max_wait_s`; tasks on agents keep running per `tasks.session_end` |
| One agent | none if other work continues; the task fails with a spoken notice (chapter 8) | never on its own |
| Voice (text-to-speech) | `stall` (pre-recorded, so it still plays) | `max_wait_s` |
| Network to the client | nothing is played (no one hears it); chapter 3's reconnect grace applies | as today |

## Decisions made and what was given up

| Decision | Chosen | Given up |
|---|---|---|
| How lines are produced | rendered in advance in the session's voice and cached; operator recordings win | generating them when needed (the generator may be what failed) |
| Who triggers them | health signals from the existing code paths | the caller's words or a model's decision |
| Honesty | "I'll get back to you" only with a follow-up channel; no vendor names | a reassuring promise nobody keeps |
| Limits | at most 3 stalls, 8 s apart, exit after 30 s by default | waiting indefinitely |

## What changes in the code

| Where | Change |
|---|---|
| language packs, persona compiler | the `failure` group (English reviewed; other languages after native review) |
| `core/interaction/clips.rs` | render, check, cache and load the failure lines with the clip bank; the server's default set |
| `core/interaction/actor.rs`, `policy.rs`, `floor.rs` | the health watcher; stall, back and exit timing at slots; history notes |
| `core/interaction/config.rs` | `failure.stall_after_ms`, `failure.max_wait_s`, `failure.lines` overrides |
| `core/interaction/events.rs` | `session_failed`; `failure` in `interaction_updated` |

## Tests to write first

| Test | What it proves |
|---|---|
| `failure_lines_are_rendered_once_and_cached_by_voice` | cache and reuse |
| `a_silent_or_long_failure_line_is_replaced_by_the_default_set` | checks |
| `a_stalled_answer_gets_a_stall_line_then_the_answer` | stall and back |
| `stall_lines_are_spaced_rotated_and_capped` | limits |
| `an_outage_past_max_wait_plays_the_exit_line_and_ends_the_session` | exit, `session_failed`, push |
| `the_exit_line_promises_a_follow_up_only_when_there_is_one` | honesty |
| `a_failing_voice_still_plays_its_recorded_stall_line` | text-to-speech down |
| `a_failing_recogniser_plays_the_hearing_line` | speech-to-text down |
| `failure_lines_never_start_over_the_caller` | floor |

Live: **SF1** kill the front model's server mid-call (a stall line, then the back line after the supervisor restarts
it); **SF2** make the background LLM stall (a stand-in that holds its answer) for 40 s (stall lines, then the exit line
and `session_failed`); **SF3** kill the local text-to-speech mid-call (the recorded stall line still heard in the
session's voice); **SF4** kill the streaming recogniser stand-in (the hearing line, then exit).

## What ships in which release

With the orchestration track (chapters 8 and 9).

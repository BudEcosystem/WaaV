# To be fixed: issues found while benchmarking and building the launch demo

Each entry: what was seen, where, and why it matters. Newest at the bottom of each section.

## Gateway: interaction mode

1. **The first turn's acknowledgement waits for the session's warm-up.** In a session whose warm-up
   lines (the acknowledgements, then backchannels and failure lines) were still being synthesised,
   the first turn's "Sure, I'll find out." played 2.5 s after the caller stopped (FD-bench pilot,
   27 Oct, Deepgram aura-2). A caller who speaks in the first ~5 s of a call waits. Synthesise the
   acknowledgements first and let a turn preempt the rest of the warm-up queue.
2. **A finished answer waited ~9 s to be spoken.** With Claude Opus 5.5 as the background, the
   task's answer was ready 1.7 s after the typed turn, the acknowledgement played at 2.4 s, but the
   relay began only at 10.9 s (first session after a gateway restart, 6 s after `ready`). Likely
   behind the warm-up/backchannel syntheses on the same voice. Investigate the speech queue order.
3. **Warm-up log lines carry no session id.** `interaction warm-up line synthesised (clip) utt=N`
   has no `stream_id`, so a client or harness cannot tell which session is warm.
4. **End-of-turn detection takes ~0.8 s after the caller stops.** In the pilot the turn ended
   (source SmartTurn) about 0.8 s after the last voiced sample; the front then decided in 135-144 ms.
   Competitors report 0.40 s (TML), 0.57 s (Gemini 3.1 Flash Live minimal) turn-taking latency on
   FD-bench v1. Tune the silence needed before Smart Turn is asked, and use the eager path more.
5. **`floor_state` flaps several times a second during one line.** One acknowledgement produced ~20
   `floor_state` events alternating `front_ack`/`relay` within 3 s. Noisy for clients; coalesce.
6. **Shutdown warnings on every session close.** `Received speech_final after CloseStream` and
   `task did not finish during shutdown grace; aborting task=streaming-tts-recv-task` on each close.

## Gateway: configuration and providers

7. **`stt_config.punctuation` is required.** A `/ws` config without it is refused with
   `Invalid message format: missing field punctuation`; it should default (the docs' own examples
   omitted it until PR #47).
8. **The Anthropic adapter is behind the current API.** `reasoning_effort` maps to
   `thinking: {type: "enabled", budget_tokens}`, which Claude Opus 5.5 / Sonnet 5.5 / Opus 5 reject
   with a 400 (they take `{type: "adaptive"}` + `output_config.effort`), and since 2026-10-21 the API
   rejects `temperature`/`top_p`/`top_k` on every model, which the adapter still sends when set.
   (Claude Opus 5.5 works as a background today only because neither is set.)
9. **`gateway/docs/SUPPORTED_PROVIDERS.md` has drifted from the code.** It lists a Sarvam
   text-to-speech, a Zalo speech-to-text and Acapela/CereProc speech-to-text that do not exist, and
   says "Realtime: 2 implemented" (the code has 11). The provider modules are the truth (31 STT, 34
   TTS + self-hosted, 11 speech-to-speech, 44 vendors).

## Distribution

10. **The release image needs a registry login.** `registry.bud.studio/runtime/waav:1.1.0` is not
    anonymously pullable, so the public README can only offer the binary archive or a local build.
11. **The SDKs are not published.** Neither `bud-waav` (PyPI) nor `@bud-foundry/sdk` (npm) exists.
12. **22 broken relative links** in the repository's Markdown (pre-existing on main, e.g. the 1.0
    README's `LICENSE` link, now in `docs/archive/README-1.0.md`).

## Documentation (found while writing the paper, 27 Oct)

13. **Design chapters still describe superseded front-model decisions, with no banner.** Chapters 1, 2, 4
    and 7 of `docs/duplex-agent/`, `contracts.md`, `front-model/README.md` and `technology-register.md` say the
    Release 1 front is the `scripted` rule router everywhere, that LFM2.5-1.2B is the reference model and that
    the act budget is 300 ms. Release 1.1.0 requires Qwen3.5-2B with no rule-based fallback and
    `front.total_budget_ms` 1500 (`interaction_mode.md`, tutorial, chapter 9). Only chapter 9 says it wins,
    and only over chapters 4 and 8. `contracts.md` also still says `gateway/src/core/` has no `interaction`
    module. A reader of the public repo cannot tell plan from product; add a "superseded by" note per chapter.
14. **Silero VAD version disagrees.** The README and the code (`gateway/src/core/silero_vad/*.rs`) use
    Silero VAD v5 (512-sample chunks at 16 kHz). Chapter 5, `technology-register.md` and `FILE_MANIFEST.md`
    say Silero v6.2 was chosen.
15. **The LiveKit text turn detector: register says removed, code and README still use it.**
    `technology-register.md` lists it as "Rejected and removed (licence)" under the LiveKit Model License
    (LiveKit Agents only). The README's audio-intelligence table runs it "in release builds", and
    `gateway/src/core/turn_detect/config.rs` still downloads `livekit/turn-detector`. Either the licence
    question is open or the register is stale. It matters for the "open source, self-hosted" claim (the paper
    tells operators to check the licence).
16. **Backchannel timing: chapter 5 and the shipped pack disagree.** Chapter 5 says gap 2,500 ms, 1 in 5
    pauses skipped, 700 ms of caller speech first, probability 0.8. The reference's English pack is gap
    6,000 ms, every third pause skipped, 1,500 ms first.
17. **Task states: nine in the design, eight on the wire.** Chapter 3 says nine A2A-aligned strings,
    including `auth_required`, are "the wire states of every client event". The reference's `state` list has
    eight and no `auth_required`: an A2A sign-in becomes an `auth_url` question instead.
18. **The duck figure in Limits reads like a second default.** "ducked at the caller's first sound (10 dB
    down within 0.1 to 0.35 s live)" sits next to `duplex.duck_db` -15 dB. Say that 10 dB is the measurement
    threshold, not the setting.
19. **`launch/research/refs.bib` line 3 breaks BibTeX.** The comment "% Web pages use @misc ..." is parsed
    as an entry, since BibTeX has no comment syntax, and gives one error. It is fixed in the paper's copy
    (`launch/paper/refs.bib`) but not in the research file.

## Found during the FD-bench v1 runs

20. **Warm-up lines are synthesised again by every session.** Each new session synthesises its ~6
    warm-up lines (acknowledgements, listening sounds, failure lines) through the TTS vendor one by
    one, about 1 s each with Deepgram aura-2, though another session of the same voice made the same
    lines seconds before. ~6-10 s per session before the agent is fully warm, and vendor cost on every
    call. Share the clip bank across sessions of the same voice (the process-wide speech cache exists).
21. **Front decisions slow down sharply with a few sessions at once.** Three concurrent sessions on one
    8-slot Qwen3.5-2B server (shared GPU, host load ~47): first bytes 410-941 ms (headers 191-253 ms)
    against 135-182 ms alone. Measure and size slots per session; consider a front server per N sessions.
22. **A mid-sentence fragment can start a task.** In FD-bench candor_turn_taking/1, the recogniser's
    final "10 companies." (the caller had not finished) was decided as `delegate` and started a task,
    1.1 s into a 6 s turn.
23. **Smart Turn misses its 50 ms deadline on a loaded host.** During the FD-bench v1 run (host load
    average 40-50 from other workloads) the log shows `SmartTurn inference timed out after 50ms`
    (51 ms against a 15 ms target). A timed-out check falls back to the recogniser's end of turn, which
    is slower. Give the turn models their own threads/cores, or let the deadline follow the measured
    inference time, and report the fallback rate as a metric.
24. **A slow vendor connection fails the whole session at start.** On a saturated link, 81 of 119
    FD-bench sessions got `Failed to start voice manager: STT error: Connection failed: Connection
    timeout` and no `ready`; the session is not retried and does not fall back to another configured
    recogniser. Retry the connect with backoff within the ready budget, and/or allow a fallback STT.
25. **Spoken text is split at the dots of version numbers.** A background answer "Bud WaaV release 1.1.0
    is published..." was spoken as "The Bud WaaV release 1." followed by "The details are on your screen."
    The sentence splitter of the relay/update treats "1." in "1.1.0" as a sentence end. Do not split
    inside a number (digit '.' digit), dates, decimals or abbreviations.
26. **The orchestrator's first words and its final answer are glued into one result.** The request's
    `task_result.summary` read "I'm checking the WaaV 1.1.0 release page on GitHub now. I'll tell you if
    it's live ... as soon as I hear back. Yes, WaaV 1.1.0 is live..." (its pre-delegation sentence and its
    answer after the child's result). The summary should be the answer only.
27. **A webhook question's `expires_in_ms` is read back to the caller as an argument.** A webhook
    `question` (kind `confirm`) with `action`, `arguments: {title}` and `expires_in_ms: 60000` was read as
    "To confirm: save the document and open it, 60000, Bud WaaV 1.1 Is Here." The deadline must not be
    part of the read-back.
28. **Every update said "The details are on your screen."** With `tasks.announce: "tell"` and results of
    two or more sentences, each update was the first sentence (cut at the version-number dot, see 25)
    plus "The details are on your screen." even on a voice-only `/ws` client that never showed them. Speak
    a short summary instead, or only point to the screen when the client has one.
29. **"That's amazing. Thank you!" after the tasks ended was decided as a request.** The front acked
    ("Sure, let me check.") and the background answered "The terminal is still garbling commands, so you
    can order more later." A closing thanks should be `social`, not a new task.
30. **A question was not repeated before it expired.** The writer's confirm was read once at 39.9 s and
    once more at 64 s; the caller's "How's it going?" got "Sorry, was that a yes or a no?", and the
    question timed out at 60 s, after which the task said "it went on without it". Consider asking once
    more right before the deadline, and saying which question a "how's it going" did not answer.
31. **One long request with a natural pause became two requests and duplicate work.** The caller's
    request (one breath, a short pause before "Mention the new interaction mode...") was decided as two
    turns: t1 and t2. The orchestrator then started two writers (t3, t6) for the same announcement, and the
    caller heard "Hi, how can I help?" and "You're welcome." in between. A pause inside one request should
    not end the turn; a second request that only adds detail should amend the first.
32. **"Could you talk a little faster? I'm in a rush today." became a writer task** (t7, "About the Bud WaaV
    version 1.1 launch announcement you just...") instead of the pace control (`front.caller.pace`).
33. **Status lines quote raw request fragments.** "Still working on your request to check the release is
    live on GitHub? Make sure." The task title is cut from the caller's words mid-sentence; titles need to
    be a clean summary.
34. **"A question about your request could not wait for your answer, so it went on without it."** was said
    for a confirm that the caller had answered "Yes, go ahead." ~10 s after it was read out (the answer
    came while two questions of two writers were open). The answer should go to the question it follows.
35. **A finished child's update was never spoken.** In take 4 the browser agent's result (t2, "Version
    one point one point zero is live and marked Latest...", 32.4 s) arrived while the orchestrator's relay
    and the writer's confirm were queued; no `update` for t2 ever played (the writer's and the terminal's
    did). With `announce: tell` every child's result should reach the caller once.
36. **Updates are the front model's rewrite, and it reintroduces digits.** The terminal's result said
    "Version one point one point zero passed the checksum..." but the spoken update (source `front`,
    verbatim false) was "The Bud WaaV 1. The details are on your screen.": the front wrote the task
    title's "1.1.0" and the splitter (25) cut it.
37. **"How's the binary test going?" was decided as a stop of that task.** In take 7 the caller's status
    question cancelled the terminal agent's running task ("Okay, I'll stop that." ... "It's stopped.") and
    a new one was started. A status question must never cancel work.
38. **A failing speech synthesis loops forever.** After network trouble (`service failing` recogniser,
    background, then voice), a `speak` command failed instantly (`ms=0`), the voice was marked failing and
    back 0.3 s later, and the same thing repeated every ~12.6 s for 8 minutes: the caller heard "Sorry, I
    couldn't get that done just now." 38 times. Retry with backoff, cap the repeats of the failure line,
    and drop a line that keeps failing.

## Found during the FD-bench v3 run (27 Oct, Claude Opus 5.5 background, Deepgram nova-3 + aura-2)

39. **Claude Opus 5.5 as background returns no answer with the default token cap.** With no
    `conversation_config.max_tokens`, the background gets the 256-token voice cap
    (`VOICE_DEFAULT_MAX_TOKENS`). Opus 5.5 always thinks (adaptive; it cannot be disabled) and thinking
    counts toward `max_tokens`, so the run ended `finish="max_tokens" content_chars=0 calls=[]`, the task
    failed with "the model returned no answer" and the caller heard "Sorry, I couldn't get that done just
    now." Setting `max_tokens: 4096` fixed it. Default an adaptive-only Anthropic background to the
    reasoning tier's 4096 (as `REASONING_DEFAULT_MAX_TOKENS` does for the S2 tier), or warn at config time.
40. **The interaction background drops the LLM's token usage.** `interaction/background.rs` builds its
    completions with `usage: None` and logs no token counts, so a Claude-backed session has no cost
    record (the FD-bench v3 report could only estimate Claude spend). Record usage per task round.
41. **A thinking background is announced as a failure.** In 29 of 100 FD-bench v3 sessions the caller
    heard "Sorry, my systems are acting up. Bear with me a moment." (`failure` line `stall-1`, service
    `background`, `degraded: background_stalled`) because Opus 5.5's first token came after
    `failure.stall_after_ms` (2.5 s). Normal think time is not an outage: start the stall clock after a
    model-specific expected latency, or say a holding line instead of a failure line.
42. **A request split by a pause re-runs the first part's tool calls.** When a caller's pause ended the
    turn early (see 22, 31), the second turn became task t2 whose run saw the whole call so far and
    called the first part's tools again: e.g. `track_order BOB12` twice, `get_card_benefits travel` twice,
    `get_exchange_rate` repeated. 13 of the 69 strict failures in FD-bench v3 are such duplicates. With a
    side-effecting tool this is a double booking. A second task should get only its own request (plus
    what t1 already did), or t1 should be revised instead.
43. **An answer said before the question is heard is lost, and was read as a control.** In ecommerce_01
    the background asked "What's the order ID ...?" (task t1 started on the first half of the request);
    the caller's "The order ID is a b c one two three." came 0.5 s before the question was played, the
    front decided it as `control` ("Sorry, I don't have that to say again."), and the question then timed
    out after 20 s. Words that answer an open question that is about to be asked should bind to it.
44. **Listening sounds count as interruptions and pollute the transcript.** With `duplex.backchannel`
    `auto` (on for a `/ws` client), 49 of 100 FD-bench v3 sessions played a listening sound before the
    caller's end of speech, so the benchmark scored 53% interruptions (9% with those sounds muted
    post hoc), and Parakeet transcribed the Aura-2 clip as "Ma'am", "Mom" or "Mim" at the start of the
    agent's transcript. Check how the rendered "mm-hm" clip sounds per voice, and document that the
    default is on for socket clients.
45. **Front decisions that miss the budget make the agent ask the caller to repeat clear speech.** In the
    first pass (2-3 concurrent sessions, host load average 70-86 from other jobs) 52 of 177 turns were
    `RuleOverride("front_failed")` and the caller heard "Sorry, I didn't catch that. Could you say it
    again?" for a clear request; 34 of 100 sessions were affected and re-run. Related to 21; the degrade
    path should delegate the turn to the background rather than discard it.
46. **Streaming TTS slower than real time is played with gaps.** Deepgram's streaming TTS delivered at
    0.2-0.8x real time from this host (measured directly: 2.1 s of audio in 10.1 s), and the gateway
    played each chunk as it came: "Both orders are out for delivery." spread over 7.3 s with silences
    inside words, and the stall line (41) fired for the voice. Buffer when the vendor is below real time
    (start after enough audio is queued), or fall back to the request-response path.
47. **The background asks for a form on a voice-only call.** 11 `task_question` events of kind `form`
    ("I'm putting a secure form on your screen for the details.") were asked of a `/ws` client that
    shows no forms, for plain details such as two addresses; all timed out. With `hitl.channels`
    defaulting to `voice` + `client`, a socket client is assumed to have a screen; ask by voice unless the
    client says it renders forms.
48. **A revised task runs its tool calls again.** When the caller's next words join a running request
    (a hesitant request, a new `revision`), the new revision's run calls the same tools again although
    revision 1 already did: finance_01 `get_exchange_rate(500 USD->EUR)` at 11.9 s and 13.6 s,
    ecommerce_06 `add_to_cart(K2, 2)` at 16.1 s and 18.4 s (one task, revision 2). 7 FD-bench v3 failures.
    For tools without `side_effects` nothing holds them (the benchmark's harness declares none, since
    nobody can confirm), so an `add_to_cart` really ran twice. Give the new revision the earlier calls and
    their results, or replay identical calls from them.
49. **Child results lost and one result spoken twice (take 15).** With three children finishing within
    20 s (terminal 27.9 s, browser 32.0 s, writer 48.6 s), only the writer's update was spoken, and twice
    (56.5 s and 64.4 s, same words); the terminal's and the browser's results were never told. The
    64.4 s utterance carries the browser task's id (`audio_segment` u_20, task t2) but speaks the writer's
    text, so the front's update for t2 was filled from the wrong task. Every finished child should be
    told once, in its own words (see 35).
50. **A follow-up about a finished child went to the wrong agent.** After the terminal child had returned
    the download counts (27.4 s), "So what are the download numbers?" (112 s) started a new writer task,
    which answered "I don't have WaaV's download numbers". A follow-up should be answered from, or routed
    to, the child whose result holds the answer.
51. **"Yes, go ahead." after a read-back is sometimes not understood.** In takes 11 and 16 the caller's
    "Yes, go ahead." (0.45 s after "Do you want me to do that?" was heard) got "Sorry, was that a yes or a
    no?", the re-ask got the same answer, and the confirm timed out; in takes 4, 8, 10, 12, 13 and 15 the
    same words worked. Likely front-model timeouts under GPU load; a plain "yes" should not need the model.
52. **The front answered a three-part request with a canned self-introduction.** Take 17: "It's launch day
    for WaaV one point one, so could you check the GitHub release, pull the download numbers in the terminal
    and draft the announcement?" was decided as `answer`: "You've reached this help line. I'm the assistant,
    an automated assistant." followed by "Sorry, I didn't catch that." on the next turns. A request with
    three imperatives should always be `delegate`.
53. **A `social` decision near the front's budget is never spoken (FD-bench v1 turn-taking).** In 7 of the
    26 Candor turn-taking samples where the agent did not take the turn, the turn settled as `social` but no
    reply was heard in the following 3-4 s; in 5 of them the front's first call took 1,457-1,501 ms (its
    1.5 s budget) under GPU load (e.g. sample 35: end of turn 19:49:00.04, `front language ms=1501`, `turn
    settled act="social"` 19:49:01.67, nothing spoken by 19:49:04.79). A settled social act should be spoken
    or fall back to a short template reply at once, not depend on a second slow front call.
54. **Storytelling read as "keep listening" (FD-bench v1 turn-taking).** 13 of the 26 misses settled as
    `wait` after a complete sentence or a direct question at the end of a speaker's story (e.g. sample 11
    "...be a good one. Right?", sample 58 "Contamination at all?"). Holding through pauses is intended, but a turn that ends
    in a question to the agent should be answered.
55. **Interruptions answered with "Sorry, I didn't catch that." (FD-bench v1 user interruption).** Of 181
    judged replies to a user who interrupted the agent with a new question, 58 were rated 0: 32 were "Sorry,
    I didn't catch that. Could you say it again?" (the front missed its budget under GPU load), 14 kept
    answering the earlier question (e.g. sample 149: asked to switch from science to arts, the agent went
    on listing science subjects), 10 were the canned self-introduction of item 52 and 2 were holding lines.
    Mean latency to the reply was 2.21 s. An interruption that asks a new question should drop the old
    answer and decide on the new words; a missed budget should not cost the caller a repeat.
56. **Root cause of 49: the narrator's limits disagree, so real results are dropped.** The narrator prompt
    asks for "at most 20 words", its grammar allows 160 characters and stops at the first full stop, and
    `guard::narration` blocks more than `NARRATION_MAX_WORDS` (20). Take 15: the terminal's line "The x86
    Linux build has zero downloads, the ARM Linux build twelve, and the checksums file fourteen, so the
    builds total twelve." (22 words, 338 ms) was blocked, and the three retries wrote the same line
    (`update not worded ... tries=1..3`, gateway.log 18:40:44-52), so the update was dropped. The browser's
    line stopped at the dot inside "1.1.0" ("The GitHub release for version 1.") and its "1" is not in the
    result ("one point one point oh"), so it was blocked too. Fix: one limit in all three places (grammar
    by words, or a guard that trims instead of blocking), a grammar that does not end at a decimal point,
    and a retry that tells the model why the line was refused.
57. **Root cause of the repeated update: a narrated line is not checked against its own task.**
    `spoken_update` only blocks numbers and names that are not in the result. The narrator also gets the
    last two lines of the call, and for the browser's update (task t2) the 2B front copied the line the
    agent had just said about the writer ("The launch announcement has been saved..."), which has no
    numbers or names and passed. `carries_result` (policy.rs) already checks that a line says something of
    the result; apply it to updates too, and refuse a line that repeats one of the last lines said.
58. **A relay sentence cut by a question is replayed whole.** Take 15: "One's checking the GitHub release,
    one's pulling the download numbers in the terminal, and one's drafting the announcement." was heard
    from 30.2 s until the confirm cut it at 34.05 s (`heard: partial`, most of it said), then said again in
    full at 45.5 s after "Okay. Anyway,". A sentence heard nearly to its end should be dropped or resumed
    from where it was cut, not repeated; the "Anyway," bridge makes the repeat more noticeable.
59. **Template lines do not look at what was just said.** Take 15: "Thanks for waiting, I'm back." (20.2 s)
    and then "Still working on it, thanks for waiting." (22.3 s). Holding and recovery lines should skip a
    phrase said in the last few seconds (the front's decisions already see the last three agent lines;
    the templates see none).
60. **After a task has finished, every caller turn is read as `follow_up`.** Rehearsal (28 Oct, stream
    launch-day-1793141908, gateway 1.1.0): once the browser task had finished, turns 6-9 all settled as
    `follow_up`: "Oh, and while you're at it, grab the download numbers in the terminal.", "How's it
    looking?", "Let's draft the launch announcement and mention those numbers." and "Thanks, Ava." No
    terminal or writer task was started; the answer narrator replied from the finished tasks' results with
    lines such as "We support three speech vendors with release notes mentioning interaction mode." (made
    up), and one turn ended in "Sorry, I couldn't get that done just now." The front prompt's examples
    teach `follow_up` after a finished task ("What time is the table for?", "Can it be on the terrace?")
    but have no new request or thanks after one. Add those examples (a new request naming other work is
    `delegate`, thanks is `social`) and gate them on the offline set.
61. **A synthetic caller's pause after "Can you" split a request.** Rehearsal: "WaaV one point one went out
    last night. Can you | pull up the GitHub release..." ended the turn after "Can you" (8.4 s), the
    orchestrator answered the fragment ("You got cut off after 'can you'"), and the rest became a second
    task. Same family as the FD-bench v3 early turn ends: a turn ending on an auxiliary plus "you" should be
    held longer.
62. **A slow voice turned into a six-minute loop of "Sorry, I couldn't get that done just now."** Take 22
    (28 Oct, gateway built from PR #49 at 6d993a5, stream launch-day-1793146049): one Deepgram synthesis
    took ~5.8 s to its first byte (`bottleneck="tts_ttfb"`), the voice was marked failing, and from then on
    every speak command flipped the voice to failing (at `speak ms=0`) and back (0.3 s later), with the
    `failure.retryable` line said every ~13 s from 91 s to 458 s although no task had failed. It also cut
    the save read-back ("Quick question." ... "Anyway,") so the confirmation was never heard. Under
    investigation on PR #49 (whether the PR caused or exposed it).
63. **A request fragment went to the finished task's agent.** Take 22: "Can you also get | the download
    numbers in the terminal?" split, and the second fragment was handed as a follow-up to the finished
    browser task (a new browser task: "Which project do you mean...?"). "How's it looking?" with no task
    running also became a browser task. The terminal only started six minutes later.
64. **The narrator garbled a result with several numbers (take 27, PR #49 build 3a3e6b2).** The terminal's
    result "For release one point one point zero, the x86 build has zero downloads, the ARM build twelve, and
    the checksum file fourteen" was told as "The release download counts are now available: one point one
    point zero for the x86 build, twelve for ARM." The number check passed (every number is in the result)
    but the pairing of number to item was wrong; the orchestrator corrected it 25 s later. A spoken update
    of a result with several numbers should keep each number next to what it counts (or relay the result's
    own sentence).

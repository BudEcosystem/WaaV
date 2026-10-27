# Chapter 9. Conversation controls: one protocol between the caller, the models and the gateway

Added 2026-10-16 at the product owner's request. It extends chapter 4 (policy) and chapter 8 (orchestrator); where they
differ, this chapter wins for the items it names.

## What this part is and why it exists

Callers do not only ask for things; they also steer the conversation itself: "stop talking", "sorry, I didn't hear
you", "say that again", "what was the number?", "slower please", "talk to me in Chinese", "louder", "keep it short",
"hold on a second", "go on". Today the gateway handles a few of these, each its own way: phrase lists for stop, repeat,
cancel and side talk (`assets/guard_lexicon.en.json`), the caller's pace and verbosity only as client settings
(`front.caller.pace`, `front.caller.verbosity`), and a turn in another language is answered with "could you say that
again?" and finally a refusal. Nothing tells the orchestrator or an agent what the caller asked for or what changed, and
an agent that wants to change the conversation (switch language because the caller wrote in Chinese) has no way to.

This chapter makes every such request one **control**: a small typed message with a closed list of kinds. Whoever
recognises it (the gateway's own phrase lists, the front model, the orchestrator or an agent, or the client app's
buttons) produces the same control, and one executor in the gateway applies it according to what the session can do,
reports what it did, and records it in the conversation history and the conversation state that every model and agent
receives.

## How it works

### The controls

| Kind | Arguments | Examples | What the gateway does |
|---|---|---|---|
| `stop` | none | "stop", "stop talking", "enough" | cut the agent now (chapter 5's cut); the cut is marked in the history |
| `hold` | optional seconds | "hold on", "give me a second" | stop speaking, keep tasks running, wait up to the time or until the caller speaks; then a check-in |
| `resume` | none | "go on", "continue", "carry on" | continue the cut or held answer from its first unheard sentence |
| `repeat` | `what`: `last` (default), `answer`, `slot` with a name (`number`, `time`, `name`, `address`, `code`) | "say that again", "what was the number?" | replay the last line the caller heard, or only the named item, slower for one line |
| `not_heard` | none | "sorry, I didn't hear you", "you cut out" | as `repeat`, plus one step louder and slower for that line |
| `clarify` | none | "what do you mean?" | the front or the orchestrator rephrases the last answer more simply |
| `spell` | optional slot | "spell that", "how do you spell it?" | the slot, letter by letter or digit by digit |
| `rate` | `slower`, `faster`, `normal` | "slow down", "talk faster" | speaking rate one step (15%) within the operator's bounds, from the next line, where the voice takes a rate |
| `volume` | `louder`, `quieter`, `normal` | "louder please" | gain in the playback pump, one step (3 dB) within bounds |
| `verbosity` | `brief`, `normal`, `detailed` | "keep it short", "tell me more" | the front's and orchestrator's instruction; "tell me more" also relays a full answer (chapter 8) |
| `language` | BCP-47 code | "talk to me in Chinese", "en español por favor" | switch the session's language (below) |
| `voice` | optional `gender` or name | "can you use a male voice?" | another voice of the same provider and language, where allowed |
| `backchannels` | `on`, `off` | "stop saying mm-hm" | the listening sounds of chapter 5 |
| `updates` | `ask`, `tell`, `screen`, optional task | "don't interrupt me with updates" | chapter 8's `tasks.announce`, for the session or one task |
| `to_screen` | optional slot | "text me that", "send it to my screen" | the last answer or slot as a `task_output`-style client event |
| `end` | none | "bye", "that's all" | the closing line, then the session ends as today |
| `human` | none | "let me talk to a person" | the operator's transfer (SIP refer or a client event) where configured, otherwise an honest line |

A control may carry `scope: "turn"` (only the next line, the default for `repeat` and `not_heard`) or `scope:
"session"` (the default for the rest). One utterance may carry several controls in order ("stop, and slower please").
A control never becomes a task, and a task request in the same utterance is handled as before.

### Who recognises a control

**No static phrase checks decide what the caller meant** (product owner, 2026-10-16). Whether "stop" is a control
("stop, that's enough") or part of a request ("stop by the store on the way") depends on the context, so every caller
utterance goes to the fast front model, which decides with the conversation's context whether it carries a control.
Phrase lists remain only as examples in the front's prompt and as test data. Three paths
produce the same `Control {kind, args, scope, source, turn}`:

1. **The front model's `control` act** with its arguments (grammar-constrained like its other acts), given the
   utterance, the line the agent is saying and what the caller has heard, and the conversation state. While the agent
   speaks, the front is asked on the recogniser's interim results as well as the final, so a stop does not wait for the
   end of the caller's turn; the newest interim cancels the previous request (the pinned `llama-server`'s
   cancel-and-rewrite). It reads a recogniser's near misses ("top talking") in context. Meanwhile the acoustic first
   stage of chapter 5 still lowers the agent's voice the moment the caller starts speaking; that is a reaction to sound,
   not a phrase check.
2. **The orchestrator or an agent**, through the tool `conversation_control` (orchestrator) or a `control` event on the
   webhook and A2A wires, for example when the caller writes or speaks in another language or asks the agent itself.
3. **The client app**, with a `control` socket message (a "slower" button, a language picker), and the LiveKit data
   topic.

**No fallback** (product owner, 2026-10-16). The front model is required: interaction mode does not work without it, so
there is no rule-based path to fall back to. The same holds for every decision the lexicons made in chapter 4 (cancel,
repeat, status, continuers, amendments, side talk): the front decides them in context. The scripted front and the rule
router stop deciding anything; phrase lists survive only as examples in the front's prompt and as test data.

- **At session start**, if the front model is not ready, interaction mode is refused with the existing refusal warning
  (`interaction_refused`, reason `front_unavailable`) and the session runs as it would without the mode.
- **Mid-session**, if the front stops answering (the supervisor of chapter 2 restarts it), `interaction_updated` reports
  `degraded: ["front_unavailable"]`; the agent makes no new decisions until the front is back (the acoustic duck still
  lowers its voice when the caller speaks), and the session ends interaction mode if it is not back within the
  supervisor's restart window.
- A slow answer is not replaced by a guess: the front's latency is a qualification gate for its profile (chapter 2),
  not a timer that hands the decision to rules.

### One executor, bounded by capabilities and the operator

The executor checks each control against:

- **The operator's bounds**, `interaction.controls`: which kinds are allowed, rate bounds (default 0.7 to 1.3), volume
  bounds, allowed languages, whether agents and clients may issue controls.
- **The session's capabilities**: does the text-to-speech provider take a rate mid-session (today OpenAI-compatible and
  Deepgram), does it have a voice for the target language and gender, does the recogniser handle the language (a
  multilingual model, a language parameter, or a restart with another model), is there a language pack, is a screen
  attached, is a transfer configured.

It then applies the control, fully or partly, or not at all, and reports a `control_applied` event to the client:
`{kind, args, source, outcome: applied | partial | unsupported | refused, detail, state_version}`. When the control is
not applied in full, the agent says so honestly in the persona's words ("I can't change my speed on this line", "I can
only speak English and Spanish here") and never pretends.

### Switching language

A `language` control is applied only if the operator allows the language and the session can do all of:

- **hear it:** a recogniser for it (the same multilingual model, the provider's language parameter, or a new
  recogniser session on a model that supports it, opened before the old one closes);
- **say it:** a voice of the session's provider for the language, preferring the persona's gender and style;
- **its own lines:** a reviewed language pack (lexicons, banks, templates, listening-sound words and timing; chapter
  5's clip bank is rebuilt for the new voice with no listening sounds until it is ready);
- **the models:** the front and the orchestrator are told the new language in the conversation state.

If there is no reviewed pack, `controls.language.answers_only` (off by default) lets the orchestrator and agents answer
in the language while the gateway's own lines are kept to the minimum and spoken in the session's original language;
`control_applied` reports `partial`. Otherwise the control is `unsupported`, said honestly, and the caller memory may
record the wish (chapter 3's caller memory, opt-in).

### The conversation state

The gateway owns one versioned **conversation state**: `language`, `rate`, `volume`, `verbosity`, `voice`,
`backchannels`, `updates`, `held`, and the list of controls applied. It is sent:

- to the client in `interaction_updated`;
- to the front in its call header, and to the orchestrator and every agent with each request (a system note for the
  LLM tier; `metadata.waav_state` on A2A, webhook and MCP requests), so answers follow the language and verbosity;
- into the caller memory (opt-in) so the next call starts in the caller's language and pace.

### The history

The gateway is the source of truth for **what was said and heard**. The history it gives the front, the orchestrator
and agents contains:

- the caller's turns as recognised;
- the agent's lines **as heard**: a line cut by the caller is kept up to the last heard word with a marker
  `[cut off by the caller]`, and lines never heard are left out (the floor's heard timeline already knows this);
- one short note per applied control (`[the caller asked to speak slower; rate 0.85]`, `[language switched to es]`);
- the results and updates spoken (chapter 8).

The orchestrator keeps its own working memory (its plans, tool results, children) and receives the gateway's history as
deltas keyed by turn ids, so both sides agree on what the caller has heard. A `resume` or `repeat` uses this history,
never the model's draft.

## Decisions made and what was given up

| Decision | Chosen | Given up |
|---|---|---|
| One protocol | a closed list of control kinds, the same from every source | free-form requests to models (unpredictable) |
| Recognition | the front model decides every utterance in context, also on interim results while the agent speaks; phrase lists only as prompt examples and test data | the speed of a list match before any model call (the acoustic duck covers the wait) |
| Without the front | no fallback: interaction mode is refused at start or ends mid-session | the scripted front and the rule router as a degraded mode |
| Applying | one gateway executor, bounded by capabilities and the operator | models changing provider settings directly |
| Honesty | `control_applied` with an outcome and an honest line | silently ignoring what cannot be done |
| History | heard text with cut markers and control notes, owned by the gateway | each model keeping its own version |
| Language switch | only with a reviewed pack, `answers_only` opt-in | switching anywhere with unreviewed lines |

## What changes in the code

| Where | Change |
|---|---|
| `core/interaction/types.rs`, `events.rs` | `Control`, `ControlKind`, `ConversationState`; `control_applied`; client `control` message |
| `core/interaction/rules.rs`, assets | the rule router and the lexicon checks stop deciding anything and are removed where nothing else uses them; the phrase lists become prompt examples and test data; `front: scripted` is refused |
| `core/interaction/front_model.rs` | the `control` act and its grammar; requests on interim results while the agent speaks, cancelled by newer ones |
| `core/interaction/background.rs`, `remote.rs` | the `conversation_control` tool; `control` events on the remote wires; state in every request |
| `core/interaction/policy.rs`, `floor.rs`, `actor.rs` | the executor; repeat and resume from the heard timeline; hold; rate, volume and voice through the speech side |
| `core/interaction/clips.rs`, `config.rs` | clip bank rebuild on a voice or language change; `interaction.controls` |
| STT and TTS session code | language change mid-session (parameter or new session); voice change |
| history plumbing (`provider.rs`, the LLM client's history) | heard-only history with markers and notes |

## Tests to write first

| Test | What it proves |
|---|---|
| `every_source_produces_the_same_control` | lexicon, front act, tool and client message agree |
| `the_front_decides_a_stop_in_context` | "stop talking" while the agent speaks cuts; "stop by the store" becomes a request; no list match acts while the front answers in time |
| `a_near_miss_stop_cue_cuts` | "top talking" while the agent speaks is read as a stop by the front |
| `without_the_front_interaction_mode_is_refused_or_ends` | no rule-based fallback; refusal at start, `front_unavailable` and the end of the mode mid-session |
| `rate_steps_stay_within_the_operators_bounds` | bounds and capability |
| `repeat_replays_the_last_heard_line_not_the_draft` | heard timeline |
| `repeat_the_number_says_only_the_number` | slot repeat |
| `resume_continues_from_the_first_unheard_sentence` | resume |
| `a_language_without_a_pack_is_reported_unsupported_and_said_honestly` | honesty |
| `a_language_switch_rebuilds_the_clip_bank_and_tells_the_orchestrator` | switch |
| `the_history_has_cut_markers_and_control_notes` | history |
| `an_agent_cannot_issue_a_control_the_operator_did_not_allow` | bounds |

Live: **CT1** spoken rate changes measured on the same line, repeat, slot repeat, stop with the cut marker in the
orchestrator's next request; **CT2** a language switch to a language with a recogniser and a voice, and an honest
`unsupported` for one without a pack; **CT3** a client `control` message and an orchestrator-issued control.

## What ships in which release

With the orchestration track (chapter 8). The phrase-list path and the executor for `stop`, `repeat`, `rate`,
`volume`, `verbosity` and `hold` come first; `language` and `voice` after the clip-bank rebuild; `human` with the
telephony transfer.

## Risks and how each is handled

| Risk | Handling |
|---|---|
| A control word in normal speech ("stop by the store") | the front decides in context; no list match acts on its own |
| The front is slower than a list match | it runs on interim results and is cancelled by the newer one; the acoustic duck lowers the voice at once; its latency is a qualification gate for the profile |
| The front is down | the supervisor restarts it; the mode is refused at start or ends mid-session, reported to the client |
| The front misreads a control | the gate set of chapter 2 gains control cases (including near misses and look-alike phrases); the front profile must pass it |
| A language switch half applied | the new recogniser opens before the old closes; outcome `partial` is reported and said |
| Unreviewed lines in a new language | reviewed packs only; `answers_only` is opt-in |
| A model and the caller disagree (the agent switches language, the caller wants English) | the caller's control wins; agent controls are bounded by the operator |

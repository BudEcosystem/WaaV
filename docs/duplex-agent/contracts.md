# Shared contracts: the readable copy of `core/interaction/contracts.rs`

Written 2026-10-05 and aligned the same day with Addenda A, B, C and D of `INTEGRATION_DECISIONS.md`. **Workstreams**,
written W1 to W7: workstream 1 runtime, 2 front model, 3 background tasks, 4 policy and human in the loop (HITL), 5
duplex audio, 6 providers and endpoints, 7 evaluation and rollout. `gateway/src/core/` has no `interaction` module
yet (its directory lists `agent` to `websocket`, no `interaction`); Release 0 creates `core/interaction/contracts.rs`
with these types and a golden test per workstream (`INTEGRATION_DECISIONS.md` section 4). Precedence: `INTEGRATION_DECISIONS.md` (Addendum D over C over B over A over sections 1 to 11), then
`/home/bud/ditto/waav/research/duplex-agent/design/cross-check-consistency.md` sections 3 and 4, then
`/home/bud/ditto/waav/research/duplex-agent/design/cross-check-coverage.md`, then the owning design
(`/home/bud/ditto/waav/research/duplex-agent/design/W1-...md` to `W7-...md`). The decisions also win over the chapters
and the artefacts in this folder. Designs cite this file's sections, never sibling line numbers.

**Terms.** *E*: the confirmed end of the caller's turn. *Front*: what answers at once (the rule router in Release 1;
the Liquid model once fine-tuned and gated). *Rule router*: W2's deterministic act router, which decides every act
when no qualified model runs (the `scripted` profile). *Background*: the LLM or agent doing the task. *Eager end of
turn*: an early, withdrawable guess that the caller has finished. *Provisional run*: a background run started at E or
at an eager end of turn, later *adopted* as the task or *discarded*. *Answer marker*: the line `ANSWER:` with which a
background that can be prompted starts its final answer; only text after it is relayed. Regimes: A cascade, B
speech-to-speech provider under gateway control, C delegating provider with WaaV as background, D native duplex on
WaaV Infer. Owners are the workstreams above. *Drain window*: the time the gateway gives open connections to finish
after a shutdown signal (section 5).

**Reading the code.** Rust-like pseudocode; a comment `owner | producer -> consumers | Rn` names the first release a
session uses the type (all are defined in Release 0, 2026-10-05 to 2026-11-06). R1 "Release 1, cascade interaction on
`/ws`", streaming speech-to-text only, 2026-11-09 to 2027-01-29; R2 "Release 2, full duplex audio, the GA cascade and
durable tasks", with interaction on segmented (file-only) speech-to-text sessions, 2027-02-01 to 2027-03-12; R3
"Release 3, speech-to-speech providers, open task protocols and text-output DAGs", 2027-03-15 to 2027-04-23; R4
"Release 4, realtime voices on phone and rooms, Hindi and DTMF", scheduled at the Release 3 exit review. The windows
are Addendum D3's targets for six gateway engineers and one machine-learning engineer; the lifecycle track's first
patch is due by 2026-10-23. Code paths are relative to `gateway/src/` unless they start with `gateway/`. Section 13
lists what the addenda changed and the names they withdrew; section 14 lists what is still open.

## 1. Identifiers

```rust
pub struct TimelineMs(pub u64); // W1 | ms since session start, tokio clock | R1
pub struct UtteranceId(pub u64); // W1 | one piece of agent audio
pub struct ItemId(pub u64); // W4 | one planned Utterance; every StreamSpan of one streamed answer shares one ItemId
pub type TurnId = u64; // W5 | TurnController turn id
pub struct UserUttId { pub seq: u64, pub controller_turn: Option<TurnId> } // W1 | one caller turn
pub struct OnsetId(pub u64); // W1 | one overlap episode (= W5 EpisodeId)
pub struct TaskId(pub String); // W3 | "task_" + 32 hex, gateway-minted
pub struct TaskRef(pub String); // W3 | newtype, defined once here (Addendum C4); "t1", "t2", never reused; the only
                                // task name a model or provider sees
pub struct Revision(pub u32); // W3 | starts at 1
pub struct HitlId(pub String); // W4 type; W3 fills "q_<task_id>_<n>"
pub struct FrontDecisionId(pub u64); // W2 | one id for W1, W2, W4
pub struct ClipId(pub String); // W5
pub enum ProvisionalKey { Turn(UserUttId), ProviderCall(ProviderCallRef) } // W3 | Turn R1, ProviderCall R3
```

## 2. Vocabularies

| Contract | Values (wire spelling) | Owner |
|---|---|---|
| Front acts (`Act`) | `wait`, `backchannel`, `delegate`, `update`, `abort`, `social`, `echo`, `progress`, `question`, `refuse`; no `confirm` or `handover` | W2 |
| Reason on `wait` | `incomplete`, `continuer`, `side_talk`, `nothing` (read from R4) | W2 |
| Act to task call | `delegate` -> `delegate` with `provisional: Some(key)` (adopts the run); any other settled act -> `discard_provisional`; `update` -> `revise`, `abort` -> `cancel`, answers -> `answer`, W4 retry -> `retry` | W2, W3 |
| Task states | `submitted`, `working`, `input_required`, `auth_required`, `completed`, `failed`, `cancelled`, `rejected`, `unknown`, plus flag `cancelling` | W3 |
| Effects | `none_committed`, a list of labels, or `unknown` | W3 |
| Answer release rule | `marker` (LLM tier; a Bud agent only with a declared `background.answer_marker`, from budapp or from the operator, section 10 and Addendum D5), `direct` (DAG without a join), `whole` (relayed at completion: Bud agent without a marker, webhook, A2A, MCP, DAG with a join) | W3 |
| Answer stop reasons | `revised`, `superseded`, `cancelling`, `speech_policy`, `marker_then_tool_call`, `regenerating`, `speakable_cap`, `run_failed` | W3 (W4 maps each to a stream-stop row) |
| Delivery policies | `interrupt` (may cut the agent's own speech, never the caller), `when_idle`, `silent`, `filler` | W4 |
| Interruption classes | `interrupt`, `backchannel`, `talking_to_others`, `background`, `noise`, `echo` (non-overlap addressed speech is `interrupt`) | W5 |
| HITL kinds | `confirm`, `choose`, `value`, `form`, `auth_url`, `offer` | W4 |
| HITL outcomes | `accept` (content), `decline` (optional correction), `cancel`, `timeout`, `superseded`, `unanswerable` (reason) | W4 |
| Unanswerable reasons | `no_secure_channel`, `no_verbatim_channel`, `session_ended`, `sensitive_declined`, `language_unsupported` | W4 |
| Answered by (`by`) | `voice`, `dtmf` (R4), `client`, `timeout`, `session_end` | W4 |
| Utterance kinds | `ack`, `answer`, `progress`, `hold`, `choice_offer`, `bridge`, `relay`, `question`, `confirm`, `notice`, `offer`, `checkin`, `followup`, `greeting` | W4 |
| History category | `said`, `aside` (heard, never in the background's history), `silent` (not heard) | W4 |
| Regimes | `Cascade` (A), `ProviderSpeech` (B), `Delegating` (C), `NativeDuplex` (D) | W6 |
| Owners; truth | `gateway`, `provider`, `none` (session may say `auto`); `enforced`, `monitored` | W6 |
| Front profiles | `auto` resolves once per server: with a licence attestation (`lfm_open_eligible` or `liquid_commercial`) the qualified Liquid profile, `lfm-standard` (LFM2.5-1.2B, preferred) or `lfm-large` (LFM2.5-2.6B, Addendum B7); else a qualified `apache-standard` (candidate Qwen3.5-2B, alternate Gemma 4 E2B; Qwen3.5-0.8B dropped); else `scripted` with `front_model_scripted`. Release 1 resolves to `scripted` everywhere, attested or not (Addendum B1); a model runs only when the operator forces it with `front_model.allow_unqualified` (echo `qualified: false`). Say modes `compose`, `canned`, `free` | W2 |

## 3. Runtime (W1)

```rust
pub enum SpeechLane { Front, Main, Backchannel } // W4 plans, W1 executes, W5 gates | R1
pub enum UserInputSource { Voice, Typed, Dtmf, Watchdog }
pub struct UserUtterance { // the caller's words | W1 -> W2, W3, W4 | R1
    seq: u64, id: UserUttId, start: TimelineMs, end: Option<TimelineMs>, text: String, is_final: bool,
    overlap: Option<Overlap>, source: UserInputSource, report: Option<EndOfTurnReport> /* W5 */ }
pub struct Overlap { agent_utt: UtteranceId, lane: SpeechLane, heard_before_onset: String,
    class: Option<InterruptionClass>, onset: OnsetId }
/// Shared by W1 and W6 (Addendum C4). Irreversibility is not on the tag: it is on W4's HitlRequest.
pub enum SpeakPurpose { Ack, Progress, Question, Confirmation, Relay, Correction, Filler, Clip, Client, Other }
pub struct SpeakTag { utt: UtteranceId, lane: SpeechLane, interruptible: bool, counts_as_floor: bool, capture: bool,
    purpose: SpeakPurpose, task: Option<TaskId>, last_clause: bool } // W1 -> SpeechPort | R1
/// The one heard-time function: delivered_at + lead(transport) + transport_lag_ms(transport).
pub fn heard_at(delivered_at: TimelineMs, transport: Transport, s: &RuntimeSettings) -> TimelineMs;
pub enum PlaybackEvent { // pump (W1) or ProviderVoice (W6, R3) -> W1 | R1; first four also carry at, heard_at
    UtteranceStarted { utt }, Delivered { utt, ms: u32 }, ClauseDelivered { utt, clause: u32 },
    UtteranceDrained { utt, delivered_ms: u64 }, Stopped { report: PumpStopReport }, StreamGap { utt, at },
    Underrun { utt, gap_ms: u32 }, Shed { utt, ms: u32 }, SynthesisDone { utt }, QueueIdle { at } } // + estimated: bool
#[async_trait] pub trait SpeechPort: Send + Sync { // impl: VoiceManager (W1, R1), ProviderVoice (W6, R3); both
                                                    // implement every method, including the two marked (Addendum C4)
    async fn speak_tagged(&self, text: &str, tag: SpeakTag) -> SpeakOutcome; // Queued | Stopped | Failed(String)
    fn play_clip(&self, clip: ClipId, tag: SpeakTag) -> SpeakOutcome;
    async fn capture_clip(&self, text: &str, tag: SpeakTag) -> Result<Vec<u8>, String>; // renders W5's ClipBank
    fn mark_stopped(&self, utts: &[UtteranceId]) -> PumpStopReport; // (C4) utterance-scoped, never "all queued"
    fn set_stream_open(&self, utt: UtteranceId, open: bool);         // (C4) an open streamed relay
    async fn provider_clear(&self, utt: UtteranceId);
    fn playout(&self) -> Arc<dyn PlayoutControl>;
    fn playback_events(&self) -> mpsc::Receiver<PlaybackEvent>;
    fn boundaries(&self) -> TtsBoundaries; // PerRequest | InsideSpeak | CompletionOnly
}
pub enum CutCause { Interrupt, DeadlineInterrupt, SideTalkTimeout, PreEmpted, Client, Failure, Shutdown, StepDown }
pub enum ReturnReason { Withdrawn, Displaced, StoppedBeforeAudible, TtsFailed, StreamClosed, StepDown }
pub enum LaneEvent { // W1 executor -> W4 PolicyInput::Lane | R1
    Started { item: ItemId, at }, ClauseBoundary { item, heard_chars: usize, heard_at }, Done { item, heard_at },
    Dropped { item, heard_chars: usize, cause: CutCause, resumable: bool }, Returned { item, reason: ReturnReason } }
/// A streamed relay is ONE open main-lane Utterance (Addendum C1 item 2). W4 sends each Speak with the same ItemId and
/// the next span; an explicit close is a Speak with no clauses and last = true. After stream_gap_hold_ms of gap the
/// relay is parked (floor_state reason stream_gap) and resumes on the next append.
pub struct StreamSpan { task: TaskId, attempt: u32, seq_from: u32, seq_to: u32, last: bool }
pub enum ProvisionalOutcome { Adopted { task: TaskRef }, Discarded(DiscardReason), Off, Refused } // W1 metric only
pub enum RuntimeMsg { Duplex(DuplexEvent) /* bounded 64, drops counted */, Playback(PlaybackEvent),
    Front { decision: FrontDecisionId, event: FrontEvent }, Task(TaskEvent), TaskLagged { missed: u64 } /* -> events_since */,
    Hitl(HitlAnswer), ClientCancelTask { task: TaskId }, ClientClear, SessionReady { greeting }, /* W1-internal arms */ }
pub enum InteractionEvent { // W1 bus -> W6 (wire), W7 (meter) | R1; every variant has `at`
    FloorChanged { from: FloorKind, to: FloorKind, reason: FloorReason /* includes StreamGap */ }, // -> floor_state
    SegmentStarted { utt, lane, kind: UtteranceKind, task: Option<TaskId> }, SegmentEnded { utt, lane, status: SegmentStatus },
    AgentTranscript { utt, lane, kind: UtteranceKind, verbatim: bool, act: Option<Act>, delta: String, heard: HeardMark },
    UserTranscript { utt: UserUttId, text, is_final, overlap: bool }, OverlapNoted { user, agent, class },
    Task { event: TaskEvent }, HitlRequested(HitlRequest), HitlResolved { id: HitlId, by: ResolvedBy },
    FrontDecision { decision, act: Act, source: ActSource, task: Option<TaskRef> },
    Speculation { turn: UserUttId, outcome: ProvisionalOutcome }, // internal subscribers only, never a client
    TimerFired { key }, PlaybackComplete, ConfigWarning { code, detail }, SteppedDown { reason: StepDownReason },
    Degraded { component, code, message } }
// RuntimePorts: SpeechPort, FrontCall, TaskRegistry, PolicyEngine, DuplexEngine, TurnController (W5's ClassifiedStart
// + EndpointStop), ClipBank, CompiledPersona, Regime, FloorAuthority.
```

## 4. Front model (W2)

```rust
pub enum Act { Wait, Backchannel, Delegate, Update, Abort, Social, Echo, Progress, Question, Refuse } // first chars unique
pub enum WaitReason { Incomplete, Continuer, SideTalk, Nothing }
/// RuleOverride includes the injection_cue lexicon (commits refuse) and the repeat-request exemption (Addendum B5).
pub enum ActSource { Model, RuleOverride(RuleId), Fallback(FallbackReason), Scripted }
pub struct TaskBinding { task: TaskRef, task_id: TaskId, revision: Revision }
pub enum DelegationKind { New { related: Option<TaskRef>, label: Option<String> /* <= 6 words */ },
    Revise { task: TaskBinding, correction: Option<String> } }
pub struct ActDecision { // W2 -> W1 -> W4 (PolicyInput::Front) | R1
    act: Act, text: Option<String>, delegate: Option<DelegationKind>, task_ref: Option<TaskBinding>,
    decision: FrontDecisionId, bank: Option<BankKey>, clip: Option<ClipId>, wait_reason: Option<WaitReason>,
    source: ActSource, guard: GuardOutcome /* Pass | Blocked | Truncated */, language_ok: bool,
    timings: DecisionTimings, raw: Option<String> }
pub enum DecisionTrigger { UserTurnEnded { utterance: UserUtterance, eot_confident: Option<bool> },
    UserPause { partial: String } /* R2 */, TaskChanged { task: TaskRef } }
pub struct DecisionContext { trigger: DecisionTrigger, lines: Vec<RuntimeLine> /* W1 data */, tasks: Vec<TaskView> /* W3 data */,
    pending_question: Option<HitlId>, allowed: AllowedActs, budget: DecisionBudget, recent_lines: Vec<String>, speculative: bool }
pub enum FrontEvent { // W2 -> W1 -> W4 | R1
    ActKnown { decision, act: Act, task: Option<TaskRef>, source: ActSource, at_ms: u32 }, // once, never revised
    TaskRefKnown { decision, task: TaskBinding }, Say { decision, chunk: String, guard: GuardOutcome },
    Done(ActDecision), Superseded { decision } }
pub struct Settled { act: Act, task: Option<TaskRef>, enqueued_text: Option<String> /* None = acknowledgement skipped */ }
#[async_trait] pub trait FrontModel: Send + Sync {
    async fn open_call(&self, init: CallInit) -> FrontCall; // never fails: runs scripted without slot or model
    fn classifier(&self) -> Option<Arc<dyn AnswerClassifierPort>>; // W4 stage-2 answer classification
    fn supports_speculation(&self) -> bool; fn echo(&self) -> FrontEcho; }
// FrontCall: decide(DecisionContext) -> DecisionHandle; settle(id, Settled); promote(id); discard(id); note(lines).
// W2 owns the lifecycle (Addendum B4): gateway-rendered prompt on llama-server's POST /completion; cancel-and-rewrite
// (no detach-and-append); grammar mandatory; a decision without an admission permit, or made while the persona prefix
// primes at call start, goes to the rule router, never into a queue (Addendum B3, B5).
// The rule router (W2 3.7.2, rules::route; the one router W4's FrontFailed path also calls) decides every act of the
// scripted profile, which is the Release 1 front on every deployment. Question step (Addendum D1.1), placed before the
// final `wait (incomplete)` rule: an addressed turn with no side-talk cue, no open question and no answer window, that
// the polar-question check flags (reference: detect_polar_question, front-model/front_reference.py:1117) and that ends
// in "?" or starts with a wh-word or a yes/no starter, and is not a status, social or repeat turn, routes to
// `delegate`. A fragment that ends on a dangling word keeps `wait (incomplete)` (front_reference.py:1187-1190). Seed
// decisions T11.d4 ("Will I get a reminder text?") and T16.d3 ("Is it going to be here by Friday?") in
// front-model/eval_scenarios.json, which the router met with silence, are regression cases. Gates for the router and
// for model profiles: section 9.
pub struct CallInit { session_id: String, persona: Arc<CompiledPersona>, caller: CallerProfile,
    memory: Option<CallerMemoryHandle> /* R2, only after a verified identity_token */, regime: Regime }
pub struct FrontEcho { requested: String, profile: String, licence: Option<LicenceAttestation>, qualified: bool,
    placement: Placement /* Sidecar|External|Pool|None */, say_mode: SayMode, persona: String } // -> W6 echo
pub struct PersonaSpec { // operator YAML or budapp (R1), tenant API (R2)
    id: String, version: u32, name, org, role, channel, language: LanguageTag, tone, formality, max_words: u8,
    backend_capabilities, delegate_when, do_not_delegate_when: Vec<String>,
    backchannel_intensity: Option<BackchannelIntensity> /* Off|Low|Medium|High; None = transport default */,
    backchannel_clips: Vec<ClipId>, interruptibility: Interruptibility /* Low|Normal|High */, utc_offset: Option<UtcOffset>,
    interruption_notes, social_scope, banks: PersonaBanks, template_overrides: BTreeMap<TemplateId, String> /* W4 validator */,
    lexicon, voice: PersonaVoice, protected_terms }
/// The four client-settable fields are preferred_name, verbosity, pace, language (Addendum C5). The identity token is
/// not a caller field.
pub struct CallerProfile { preferred_name: Option<(String, NameSource)>, verbosity, pace, language,
    formality, pronunciations, accessibility, memory_facts: Vec<MemoryFact> /* <= 3; server side only */ }
pub struct CompiledPersona { spec: Arc<PersonaSpec>, front_system: String, prefix_hash: [u8; 32],
    background_preamble: String /* W3 */, provider_instructions: String /* W6 */, tts: TtsSettings, banks: CompiledBanks }
pub struct CallerKey(String); // "waav:mem:" + hex(HMAC-SHA256(identity key, tenant || 0x00 || opaque id)); id never stored
#[async_trait] pub trait CallerMemory: Send + Sync { // R2, off by default, in W3's task Redis
    async fn read(&self, key: &CallerKey, max: usize) -> Result<Vec<MemoryFact>, MemoryError>; // call start, 150 ms
    async fn write(&self, key: &CallerKey, facts: Vec<MemoryFact>) -> Result<WriteReport, MemoryError>; // after the call
    async fn forget(&self, key: &CallerKey) -> Result<(), MemoryError>; }
// Identity (Addendum C5): an operator-signed token in interaction.front.caller.identity_token, one format pinned by W2,
// verified with front_model.memory.identity_key_file. A bad token: config_warning caller_identity_rejected, memory off.
// Task store in its in-memory fallback: caller_memory_unavailable. Writers: the background or the agent platform,
// never the front. In Bud mode budapp may supply the facts.
```

## 5. Background tasks (W3)

```rust
pub enum TaskState { Submitted, Working, InputRequired, AuthRequired, Completed, Failed, Cancelled, Rejected, Unknown }
pub enum SideEffects { None, Possible, Declared }
pub enum EffectsKnowledge { NoneCommitted, Committed(Vec<CommittedEffect>), Unknown }
pub struct TaskEvent { // W3 broadcast (1,024) -> W1 -> W4, W2, W6, W7 | R1
    task_id: TaskId, task_ref: TaskRef, seq: u64, revision: Revision, at_ms: u64,
    suggested_delivery: Option<DeliveryPolicy> /* W4 decides */, kind: TaskEventKind }
pub enum TaskEventKind {
    Created { title, kind, backend, side_effects, speculative: bool }, Queued { behind: Option<TaskRef>, reason },
    StillQueued { behind, waited_ms }, Working, Progress { text: Option<String>, fraction: Option<f32>, source }, Partial { chunk },
    AnswerSentence { run: u32, attempt: u32, seq: u32, text: String, first_after_e_ms: Option<u32> }, // C1: verbatim relay
    AnswerStopped { attempt: u32, released_sentences: u32, released_chars: u32, reason: AnswerStopReason },
    InputRequired { request: HitlRequest }, AuthRequired { request: HitlRequest },
    InputResolved { request: HitlId, outcome: HitlOutcome }, InputAdded { words: Vec<UserUtterance> },
    Revised { revision, kind: RevisionKind, strategy, fell_back: bool }, Retried { run }, Superseded { result_revision },
    StaleOutcome { outcome }, EffectsUpdated { effects }, Completed { result: TaskResult },
    Failed { error: TaskError, effects }, CancelRequested { reason }, CancelNeedsConfirmation { request: HitlRequest },
    CancelUnconfirmed { why, effects }, Cancelled { outcome, effects }, Rejected { reason, speakable: Option<String> },
    Unknown { why, handle: Option<BackendHandle> }, DeadlineReached { elapsed_ms, operation_running: bool },
    Orphaned, Resumed { from } }
pub enum AnswerStopReason { Revised, Superseded, Cancelling, SpeechPolicy(RuleId), MarkerThenToolCall, Regenerating, SpeakableCap, RunFailed }
pub struct TaskResult { revision, text: String, speakable: String, streamed: Option<StreamSummary> /* attempt, sentences,
    chars, complete */, data: Option<Value>, truncated: bool, cancel_raced: bool, provenance: Provenance }
pub struct TaskView { // W3 -> W1 -> W2 (renders) and W4 (templates) | R1
    task_ref: TaskRef, task_id: TaskId, revision: Revision, title: String, state: TaskState, cancelling: bool,
    cancel_unconfirmed: bool, queued_behind: Option<TaskRef>, status: Option<StatusText>, effects: EffectsKnowledge,
    stale_outcome: bool, open_question: Option<HitlId>, result_ready: bool, result_heard: HeardState,
    elapsed: ElapsedBucket /* coarse; no clocks */, side_effects: SideEffects, speculative: bool }
// TaskBoard { views: Vec<TaskView> (<= 5), open_question, version } — version ignores elapsed-only changes.
pub trait TaskRegistry: Send + Sync { // held by W1's actor; returns at once | R1
    fn start_provisional(&self, origin: ProvisionalOrigin, words: ProvisionalWords, adopt_by: Option<TimelineMs>)
        -> ProvisionalKey; // Addendum C2 item 1
    fn confirm_provisional(&self, key: &ProvisionalKey, e_at: TimelineMs); // eager start, same words at E: deadline at E
    fn discard_provisional(&self, key: &ProvisionalKey, why: DiscardReason); // idempotent
    fn delegate(&self, req: DelegateRequest) -> Result<DelegateOutcome, DelegateRefusal>; // adopts req.provisional
    fn retry(&self, t: TaskTarget, u: Option<UserUttId>) -> Result<(), RegistryError>;
    fn revise(&self, t: TaskTarget, r: RevisionRequest) -> Result<(), RegistryError>;
    fn cancel(&self, t: TaskTarget, c: CancelCommand) -> Result<(), RegistryError>;
    fn answer(&self, a: HitlAnswer) -> Result<(), RegistryError>;
    fn provider_input(&self, c: ProviderCallRef, i: ProviderIntent) -> Result<ProviderInputOutcome, RegistryError>; // W6, R3
    fn heard(&self, report: HeardReport); fn board(&self) -> TaskBoard; fn has_live_work(&self) -> bool;
    fn live_actions_without_push_target(&self) -> Vec<TaskRef>;
    fn subscribe(&self) -> broadcast::Receiver<TaskEvent>; fn events_since(&self, task: &TaskId, seq: u64) -> Vec<TaskEvent>;
    fn session_ended(&self, reason: SessionEndReason); // W1's host; a clean close skips the reconnect grace (C7);
                                                       // ServerShutdown follows the shutdown rule below (D6.3)
    fn reattach(&self, credential: Option<SessionCredential>) -> Result<(), RegistryError>; } // R2, abnormal disconnect only
pub enum ProvisionalOrigin { FrontAtE { turn: UserUttId }, EagerEndOfTurn { turn: UserUttId },
    ProviderCall { call: ProviderCallRef, adopt_by: TimelineMs } /* W6's Deepgram and Gemini speculative calls, R3 */ }
/// adopt_by (Addendum A1): FrontAtE -> E + act_ms + turn_decision_slack_ms + adoption_grace_ms = E + 600 ms;
/// EagerEndOfTurn -> None until confirm_provisional (different words or EagerWithdrawn discard the run; ceiling eager
/// start + eager_provisional_max_ms); ProviderCall -> the origin's value. Never anchored at the provisional start.
pub struct ProvisionalWords { user_words: Vec<UserUtterance>, utterance_ids: Vec<UserUttId>, snapshot: HeardHistory,
    backend: Option<String>, arguments: Option<Value> /* provider calls only */ }
pub enum DiscardReason { SettledAct, Answer, ContinuationMerge, TurnResumed, EagerWithdrawn, WordsMismatch, Replaced,
    Deadline, DelegateRefused, ProviderCancelled, StepDown, SessionEnded }
pub struct DelegateRequest { origin: TaskOrigin, utterance_ids: Vec<UserUttId>, user_words: Vec<UserUtterance>,
    front_label: Option<String>, front_summary: Option<String>, kind_hint: Option<TaskKind>, backend: Option<String>,
    arguments: Option<Value>, snapshot: HeardHistory, provisional: Option<ProvisionalKey>, e_at_ms: Option<u64> }
pub struct DelegateOutcome { task_ref: TaskRef, queued: Option<QueueReason>, behind: Option<TaskRef>, lane: RunLane }
// DelegateRefusal: QueueFull | SessionTaskLimit | SessionBudgetExhausted | UnknownBackend | BackendNotAllowed |
// UtteranceAlreadyUsed | Duplicate{existing} | Clarify{candidates} | ShuttingDown (D6.3).
// SessionEndReason: a clean close, an abnormal disconnect (W3 3.12.2) and ServerShutdown (D6.3).
// TaskOrigin: Front{decision, utterance_ids, speculative} | Provider{provider, call} (R3) | Client | FollowUp | PhoneContinuation.
// RevisionRecord { n: Revision, kind: RevisionKind /* includes Continuation */, evidence, user_words, at_ms }.
// Gate: side-effecting calls of a provisional run wait for adoption; read-only tools and generation proceed. On a
// backend that runs its own tools, `on` needs the backend declared free of side effects, else
// speculative_start_unsafe_backend (Addendum C2). Backends (DagBackend from R3) are W3-internal.
// Streamed answer (Addendum C1): text before ANSWER: is a draft, never relayed (dropped when a tool call follows:
// draft_dropped; a unit with no marker is released whole: no_marker); a marker then a tool call gives
// AnswerStopped{MarkerThenToolCall}. The LLM tier cuts sentences with core::text::SentenceAggregator
// (core/text/sentence.rs:97), as the conversation path does (core/conversation/mod.rs:1402).
// Bud agents (Addendum D5): rule `marker` only with a declared answer_marker (section 10: budapp, or the operator's
// default or alias for an agent it knows emits the marker). Without one the answer is relayed whole at completion,
// about 0.4 to 1 s later than today's agent path (W3 estimate); sign-off 13 has a person accept that explicitly, and
// W7's time-to-answer gate on the /ws agent surface covers only agents with a declared marker (the others are
// reported against the agent-path baseline).
// Shutdown (Addendum D6.3). On SIGTERM or SIGINT the gateway cancels its app-wide token AppState.shutdown
// (state/mod.rs:100-108) before the HTTP drain starts (main.rs:466-470, :531-538, :558-563); each /ws loop then sends
// `error` "server shutting down" and exits with SessionLoopExit::Shutdown (handlers/ws/handler.rs:586-599), which W1's
// host passes on as SessionEndReason::ServerShutdown. W3's task hosts and process-wide TaskSupervisor watch the same
// token and from then on start nothing new: delegate returns ShuttingDown, start_provisional starts no run (W1 records
// Refused), and an open provisional run is discarded (DiscardReason::SessionEnded). In-flight side-effecting tool
// calls run to their end within the drain window (30 s on the TLS path, main.rs:537-538; the plain path sets no bound
// of its own, so the pod's termination grace period, the time the orchestrator waits before killing the process,
// bounds it). A task still live when the window closes is recorded `unknown` with its effects as known. Release 1's
// task store is in memory, so those records do not survive the deploy; Release 2's task Redis keeps them for
// reconnects and GET /v1/tasks (W3 3.12.2).
```

## 6. Policy and human-in-the-loop (W4)

```rust
pub enum DeliveryPolicy { Interrupt, WhenIdle, Silent, Filler }
pub enum PolicyInput { // W1 -> W4 | R1
    Floor(FloorChange), VadOnset { at } /* W1 builds it from W5's DuplexEvent::VoiceOnset (C4) */,
    Speculative { turn: UserUttId, transcript: String },
    EndOfTurn { turn, transcript, onset_at, vendor_confidence, detected_language, language_ok: Option<bool>,
    class: InterruptionClass, attribution: Attribution, suspected_echo: bool, source: UserInputSource },
    FrontAct { decision, turn: Option<UserUttId>, act: Act, task: Option<TaskRef> }, Front { decision, outcome: ActDecision, late: bool },
    FrontFailed { decision }, AnswerClass { request: HitlId, turn, class: Option<StageTwoClass> }, Lane(LaneEvent),
    Task(TaskEvent), HitlClientAnswer(HitlAnswer), Dtmf { digit } /* R4 */, CapturedSpeech { at, voiced_ms },
    Consult(PolicyReason /* DeadAir|IdleCheckIn|ProgressDue|HitlTimeout */), Held(HoldNotice /* Expired|Released */),
    SideTalkHeld { item, since }, EchoWithdrawn { item }, ProviderSpeech { verdict: ClaimVerdict, at } /* R3 */,
    ClientSpeak { text, interruptible }, SessionReady { greeting }, Restarted { open_tasks }, SessionEnding { reason }, Tick }
pub enum PolicyAction { // W4 -> W1 | R1
    RequestFront { turn: Option<UserUttId>, trigger: DecisionTrigger, allowed: AllowedActs }, // W1 also starts the provisional run
    ClassifyAnswer { request: HitlId, turn }, TurnSettled { turn, act: SettledAct, task: Option<TaskRef>, adopt: bool },
    Speak(Utterance), PlayClip { clip, lane, kind }, DropQueued { item }, StopAtBoundary { item },
    Task(TaskCommand /* Create{origin_turn,..} | Revise{kind} | Cancel | Answer | Retry */),
    Provider(ProviderControl) /* R3 */, Signal(InteractionSignal), History(HistoryEntry), Redact { turn, spans },
    OpenSensitiveCapture { request, channel } /* R2 */, CloseSensitiveCapture { request },
    StepDown { reason: StepDownReason /* LanguageUnsupported: /ws conversation and agent sessions only (C7) */ },
    Metric(PolicyMetric) }
pub enum SettledAct { Act(Act), Template(Move), Answer(HitlId), Merged(TaskRef) }
pub struct Utterance { // W4 plans | W1 executes | R1
    id: ItemId, lane: SpeechLane, kind: UtteranceKind, source: TextSource, clauses: Vec<String> /* relay: pipeline bytes */,
    interruptible: bool, start: StartRule /* Now|AtClauseBoundary|WhenIdle */, expires_at: Option<TimelineMs>,
    task: Option<(TaskId, Revision)>, history: HistoryCategory, meta: UtteranceMeta /* Statement|Question|Confirmation */,
    must_be_heard: Option<ClauseRange>, bypass_interlocks: bool, reply_to: Option<UserUttId>, stream: Option<StreamSpan> }
/// The fields Addendum C4 publishes, plus ack_clip_max_chars (C6); the rest of the block is W4's table (W4 3.11).
pub struct PolicyConfig { ack_due_ms: u32 /* 300; 0 allowed */, fast_answer_ms: u32 /* 700; 0 = fast mode off */,
    ack_target_ms: u32 /* 800: the latest a generated line may be heard */, ack_hard_deadline_ms: u32 /* 2,000 */,
    answer_window_ms: u32 /* 4,000: W1's interlock and W5's rule 5 */, agreed_wait_max_ms: u32 /* 600,000: idle hang-up */,
    ack_clip_max_chars: u32 /* 24 */, /* ...W4 3.11 */ }
// Probes: answer_expected() (W5 rule 5), utterance_meta(item), holds_session_open() (idle hang-up), policy_hash().
pub struct HitlRequest { id: HitlId, task: TaskId, task_ref: TaskRef, revision: Revision, origin: HitlOrigin /* W3 */,
    kind: HitlKind, binding: Binding, proposal: Option<ActionProposal>, question: Option<VerifiedText>,
    options: Vec<HitlOption>, schema: Option<FlatSchema>, sensitive: bool, default: Option<Value>,
    expires_at: Option<TimelineMs>, approvals: Option<ApprovalMeta>, gateway_local: bool } // R1 (Form, AuthUrl R2)
pub enum HitlKind { Confirm, Choose, Value, Form, AuthUrl, Offer }
pub enum Binding { Bound { operation_id: OperationId }, BackendBound { approval_request_id: String } /* R2 */, NonBinding }
pub struct ActionProposal { verb_phrase: VerifiedText, params: Vec<ProposalParam>, consequence: Option<VerifiedText>,
    reversible: bool, template_id: Option<String> }
pub enum HitlOutcome { Accept { content: Option<Value> }, Decline { correction: Option<String> }, Cancel, Timeout,
    Superseded, Unanswerable { reason: UnanswerableReason } }
pub enum UnanswerableReason { NoSecureChannel, NoVerbatimChannel, SessionEnded, SensitiveDeclined, LanguageUnsupported }
pub struct HitlAnswer { request: HitlId, task: TaskId, revision: Revision, outcome: HitlOutcome,
    via: AnswerChannel /* Voice{utterance, vendor_confidence, attribution} | Dtmf | ClientUi */, strength: Option<Affirmation>,
    heard: HeardVerdict, question_heard: String, user_said: Option<String>, at: TimelineMs }
pub enum ResolvedBy { Voice, Dtmf, Client, Timeout, SessionEnd }
pub struct ClaimVerdict { kind: ClaimKind /* Completion|Entity{rule}|OutcomePrediction|NonceLeak */, task: Option<TaskRef>,
    heard_text: String, output_stopped: bool } // W6 ClaimMonitor -> W4 | R3
// InteractionSignal -> W6 wire: QuestionQueued, QuestionHeard, QuestionResolved { by: ResolvedBy }, Delivered, Floor.
```

Safety rules: an irreversible `accept` binds only from a `High`-attribution turn; a `suspected_echo` turn never binds;
while `answer_expected()` is true W5 classes a short answer `interrupt` and W4's answer parser runs first. W4 maps every
`AnswerStopReason` onto a stream-stop row; `RunFailed` says the answer was cut off; `MarkerThenToolCall` withdraws
what was heard.

## 7. Duplex audio (W5)

```rust
pub enum InterruptionClass { Interrupt, Backchannel, TalkingToOthers, Background, Noise, Echo }
pub enum Attribution { High, Medium, Low }
pub enum DuplexEvent { // W5 -> W1 RuntimeMsg::Duplex | R1; no stop event (stops surface once, as LaneEvent)
    VoiceOnset { at, prob, echo_score, reflex_ducked } /* W1 -> PolicyInput::VadOnset */, VoiceOffset { at, voiced_ms },
    SmartTurn { at, probability }, AnalyserDegraded { .. }, OverlapStarted { episode, at, lanes },
    GateChanged { lane, state: GateState, at }, OverlapClassified { episode, class, confidence, by, rule },
    OverlapNote(OverlapNote), SideTalkHeld { episode, utterance }, EchoWithdrawn { episode, utterance },
    Held { utterances, outcome: HeldOutcome }, BackchannelPlayed { .. } /* R2 */,
    EagerEndOfTurn(EagerEndpoint), EagerWithdrawn { turn: TurnId, at, why: EagerWithdrawal },
    EndpointDetected(EndOfTurnReport) /* E */, EndpointHeld { .. }, TurnTextRevised { turn, text }, EchoState { .. } /* R2 */ }
pub struct EagerEndpoint { turn: TurnId, seq: u8, at, acoustic_end, transcript: String,
    source: EagerSource /* SmartTurn{p} | Provider (Deepgram Flux, R2) */ }
pub enum EagerWithdrawal { NewWords, VoicedSpeech { voiced_ms: u32 }, ProviderTurnResumed, Flagged }
pub struct EndOfTurnReport { turn: TurnId, at: TimelineMs /* E */, acoustic_end, onset_at: Option<TimelineMs>,
    transcript: String, source: EndpointSource /* SmartTurn | Provider | SttSpeechFinal | SilenceCeiling | Segmented (R2)
    | ClientCommit | HardTimeout */, confident: bool /* -> W2 eot_confident */, eager: EagerMatch,
    class: InterruptionClass, attribution: Attribution, suspected_echo: bool }
/// Same -> W1 calls confirm_provisional; Different -> discard_provisional(WordsMismatch). A guess already withdrawn was
/// reported by EagerWithdrawn (its run discarded then), so it reads None at E (Addendum C3).
pub enum EagerMatch { None, Same { seq: u8 }, Different { seq: u8 } }
pub trait PlayoutControl { // on W1's metered pump; W6 ProviderPlayout (R3)
    fn set_gate(&self, lanes: LaneSet, cmd: GateCommand);
    fn stop(&self, target: StopTarget /* Utterances(Vec<UtteranceId>) | AudibleOnly */, reason: StopReason) -> PumpStopReport;
    fn hold_new(&self, lanes: LaneSet, until: HoldUntil); fn release_hold(&self, lanes: LaneSet);
    fn delivered(&self, u: UtteranceId) -> Option<DeliveredCursor>; }
pub enum GateCommand { Duck { gain_db: f32, ramp_ms: u16 }, Pause { fade_ms: u16 }, Resume { ramp_ms: u16, rewind: Rewind }, Open }
pub enum StopReason { Interrupt(EpisodeId), DeadlineInterrupt(EpisodeId), SideTalkTimeout(EpisodeId), Client, Runtime }
pub struct PumpStopReport { stopped: Vec<PumpStopped /* utterance, delivered_ms, last_delivered_at, frames_purged */>, at }
pub struct FloorSnapshot { agent_audible: LaneSet, floor_utterance_open: bool /* true across stream gaps */, paused: LaneSet,
    held: LaneSet, episode_open: bool, interruptible: bool, regime: Regime, egress_linear_pcm16: bool, bare_onset_ducks: bool }
// agent_holds_floor() = audible floor lane || floor_utterance_open || episode_open || paused floor lane (W1's probe).
pub struct OverlapNote { episode, at, end, class, confidence: f32, text: Option<String>, text_final: bool,
    task: Option<TaskId>, anchor: AgentAnchor, attribution: Attribution, suspected_echo: bool }
pub enum ClipGroup { Backchannel(BackchannelGroup /* Continue|Affirm|Understand|Empathy */),
    Policy(PolicyClipGroup /* W4: Ack|Holding|CheckIn|Bridge|FollowUp|Notice|Withdrawal */) }
pub struct ClipBank { voice: VoiceKey, clips: HashMap<ClipGroup, Vec<Clip>>, ready: AtomicBool } // R1
#[async_trait] pub trait ClipRender { async fn render(&self, v: &VoiceKey, s: &ClipSpec) -> Result<Vec<u8>, String>; }
// ClipRender: W1 via capture_clip on the session's own provider (R1); W6's ClipRenderer in regimes B and C (R3).
// Segmented speech-to-text sessions (file-only models) get interaction mode in Release 2 (Addendum D2), after the
// segmented plan's Release 1 (2026-12-18, agent sessions) and Release 2 (2027-01-29, conversation sessions). W5's
// frame-level onset tap on that plan's detector (owned by W5, reviewed by the segmented team), the
// duplex.barge_in.segmented.* values, EndpointSource::Segmented and the segmented tests take effect then. Release 1 is
// streaming speech-to-text only and needs none of that plan's TurnEvent::Aborted, DetectorSpeechStart or test kit.
```

## 8. Providers, regimes and configuration (W6)

```rust
pub enum Regime { Cascade, ProviderSpeech, Delegating, NativeDuplex }
pub struct RegimeDecision { // select_regime -> W1 host, W2, W4, W5 | R1 (A), R3
    regime: Regime, front: FrontPlacement /* Local | Narrator(Templates|FrontModel) | Provider | Off */,
    filler_owner: Owner, backchannel_owner: Owner, idle_owner: Owner,
    voice_owner: VoiceOwner /* Provider | Gateway: the half-cascade, WaaV text-to-speech is the voice; owners then
                               resolve to filler Gateway, backchannel None, idle Gateway (Addendum A2 item 8) */,
    confirmations: ConfirmPath, delegation: Delegation /* NativeClient | ViaGatewayTool | None */, truth: Truth,
    egress: Egress /* Pump | StopOnly | ProviderOnly */, warnings: Vec<ConfigWarning> }
pub enum RegimeRefusal { ProviderRetired, InteractionUnsupported { reason }, NeedsBackground,
    TruthEnforcementUnavailable, SurfaceUnsupported { surface }, TruthAckRequired { provider, model } }
pub struct ProviderCallRef { epoch: u32, kind: CallKind /* FunctionCall{call_id, name} | Delegation{id} */ }
pub struct InteractionConfig { // session envelope; owners parse their blocks | R1
    enabled: bool, regime: RegimeChoice, filler_owner, backchannel_owner, idle_owner: OwnerChoice,
    speech: SpeechConfig, hitl: HitlChannels, events: EventsConfig, runtime: Option<RuntimeSettings> /* W1 */,
    front: Option<FrontConfig> /* W2 */, tasks: Option<TaskConfig> /* W3 */, policy: Option<PolicyConfig> /* W4 */,
    duplex: Option<DuplexConfig> /* W5, never compute */ } // lenient parse, never fails
pub struct InteractionServerConfig { // the one server `interaction` key, outside ServerConfig
    enabled: bool /* W1; env WAAV_INTERACTION_ENABLED */, speech: ServerSpeech /* truth_ack */, defaults: ServerDefaults
    /* agent R1, realtime R3 */, allow_client_override: Vec<String>, realtime_legacy_wire: Vec<String>,
    tasks: TaskServerConfig /* W3 catalog and switches */, duplex: DuplexServerConfig /* W5 compute */ } // front_model: W2's own key
pub struct InteractionEcho { // ready.interaction | session_created.interaction | bud.interaction.state | R1
    protocol_version /* "1.1" */, regime, source: Vec<String>, front: FrontEcho, owners, voice_owner, truth, tasks,
    policy_hash: String, policy: PolicyConfig /* effective */, barge_in: BargeInConfig /* effective */, memory, degraded }
pub struct ConfigWarning { code: &'static str, message: String, detail: Option<Value> }
```

Regime narrowing (Addendum A2 item 7): a forced regime is honoured only when the provider can run it; the arms that
force delegation are exclusive; `auto` owners resolve to `provider` in a forced delegating regime; a cascade surface
asked for another regime gets `regime_downgraded`. `ProviderCapabilities` (W6 section 3.2, built dark in Release 0)
feeds `select_regime`, W5 and W2. It has `half_cascade_possible` (text output and a documented reply switch: only the
OpenAI family today), and `model` as the code sends it: Grok sends `grok-realtime` when no model is configured
(`core/realtime/grok/protocol.rs:49-53`), and moving that default is a lifecycle-track item behind a live probe. Azure
routes by deployment name, not model (`core/realtime/azure/protocol.rs:36-38`), so Release 3 adds an optional
`model_family`; until it is set, fillers resolve to `gateway` with `model_family_unknown`. `TaskWire` (R3: `Ack`,
`Progress`, `Question`, `ResultPart{seq, last}`, `Result`, `Cancelled{effects}`) names tasks by `TaskRef` only. In the
code today `/ws` `config_warning` has `code`, `message` and `detail` (`handlers/ws/messages.rs:383-393`),
`PROTOCOL_VERSION` is `"1.0"` (`:38`; 1.1 in Release 1), and `error` has only `message` (`:372-376`; Release 1 adds an
optional, additive `code`). **W7** owns no contract type: its `InteractionMeter` subscribes to `InteractionEvent` and
reads `policy_hash`, the effective policy and `FrontEcho` from the echo.

## 9. The agreed numbers

| Value | Agreed | Owner | Where set |
|---|---|---|---|
| Act committed after a decision request | 300 ms | W2 | `interaction.front.act_budget_ms` (may only be lowered) |
| `TurnDecisionMax`, the only outer net | `act_ms` + 200 = 500 ms | W1 | `runtime.turn_decision_slack_ms` |
| Provisional-run adoption deadline | E + `act_ms` + slack + 100 ms hop grace = E + 600 ms, anchored at E | W3 | derived; `adoption_grace_ms` (100) |
| Eager-started run | no deadline until E; discarded if E has not come 1,500 ms after the eager start | W3 | `eager_provisional_max_ms` (= W5 `endpoint.silence_ceiling_ms`) |
| Provider-call provisional run | discarded at `adopt_by` + 500 ms; `adopt_by` per provider (Deepgram 3,000 ms, Gemini 10,000 ms; estimates) (Addendum E2) | W3, W6 | per-provider `adopt_by` |
| Acknowledgement due | E + 300 ms (0 allowed); fast mode E + 700 ms when the backend's median E to first sentence is under 700 ms over 3 turns, skipped if a sentence is ready | W4 | `policy.ack_due_ms`, `fast_answer_ms`, `fast_answer_min_turns` |
| Acknowledgement source | a clip by default; a generated line only if rule 5 predicts it heard by E + 800 ms; clip lines at most 24 characters | W4 | `policy.ack_target_ms`, `ack_clip_max_chars` |
| Amendment; hard limit | E + 1,200; E + 2,000 ms | W4 | `policy.amend_ack_target_ms`, `ack_hard_deadline_ms` |
| Answer window | 4,000 ms, one value, to be measured | W4 | `policy.answer_window_ms` |
| Pre-emption wait at a clause boundary | 400 ms | W4 | `policy.interrupt_max_wait_ms` |
| Heard time | `delivered_at` + lead (LiveKit 40, `/ws` and GA 150 ms) + lag (`/ws` 150, LiveKit 80, SIP 150, GA 150 ms); confirmations + 150 ms | W1; W4 | `runtime.playback_lead_ms`, `transport_lag_ms`; `policy.heard_margin_ms` |
| Duck, pause, decide, maximum pause | −15 dB over 60 ms at onset; pause 400 ms (segmented sessions, from Release 2: 700 ms voiced; a 250 ms pause is a Release 1 listening test); decide by 900 ms; max 2,500 ms | W5 | `duplex.barge_in.*`; `duplex.barge_in.segmented.*` (R2) |
| Side talk | resume after 700 ms silence; cut at 6,000 ms | W5 | `duplex.barge_in.side_talk_*` |
| Yield backstop | computed: W5's longest episode + 1,000 ms (about 7,700) | W1 | `runtime.yield_backstop_ms` |
| Stream gap before a relay is parked | 600 ms, to be measured | W1 | `runtime.stream_gap_hold_ms` |
| Stalled answer stream; continuation merge | one holding clip after 2,000 ms; 2,000 ms | W4 | `policy.answer_stall_clip_ms`, `continuation_merge_ms` |
| Marker followed by a tool call | stopped at the clause boundary and withdrawn; gated as a rate, proposal at most 2% | W3; W7 | `waav_task_answer_marker_then_tool_total` |
| Progress | none before 4 s; 5 s apart; at most 3 per wait; expiry 3 s; W3 coalesces to one per second | W4; W3 | `policy.progress_*`, `updates_max_per_wait`; `tasks.progress_coalesce_ms` |
| Holding line; hard silence; check-in | 8 s; 10 s; every 30 s in an agreed wait | W4 | `policy.hold_after_silence_ms`, `quiet_checkin_ms`; W1 adds `runtime.thinking_backstop_extra_ms` (2,000) |
| Agreed wait before idle hang-up | 600,000 ms (sign-off 11) | W4 | `policy.agreed_wait_max_ms` |
| Question re-ask, timeout, auth wait; backstop | 8 s, 20 s, 120 s; 130 s | W4; W3 | `policy.hitl_reprompt_after_ms`, `hitl_answer_timeout_ms`, `auth_wait_timeout_ms`; `tasks.question_backstop_ms` |
| Relay pagination; stored speakable cap | 600; 1,200 characters | W4; W3 | `policy.max_relay_chars`; `tasks.result_caps.speakable_chars` |
| Front breaker | per instance 3 errors, 30 s; per call 3 act-budget breaches, 20 s | W2 | internal |
| Front qualification gates (any model profile; Addendum D1.4) | act macro-F1 ≥ 0.85, every act ≥ 0.75; delegate recall ≥ 0.98, precision ≥ 0.95; status questions sent as new tasks ≤ 1%; injection success ≤ 1%; single-call act p95 ≤ 100 ms on GPU or ≤ 200 ms on CPU, on the device class the profile will run on; act p95 within `act_ms` (300 ms) under Poisson load at the published calls per instance. Act-choice rows pass on both the teacher-labelled and the human-labelled sets; teacher labels count only once the teacher agrees with the human labels at kappa ≥ 0.7 (D1.5) | W2; W7 | release gate, not configuration |
| Rule router gate (Addendum D1.2) | at the Release 0 exit review (2026-11-06): delegate recall ≥ 0.95 on a frozen held-out set, its lower confidence bound published; the router is tuned only on a separate development split, and every checkpoint report scores it on the same sets. On the 68 seed decisions the question step (section 4) raised delegate recall from 0.92 to 1.00 and acceptable acts from 0.87 to 0.90 with no new misses (the final review's simulation). Its other seed-set numbers miss the model gates (precision 0.81 to 0.82 against 0.95; `refuse` 1 of 5; 3 of 5 injections followed before the `injection_cue` rule) and are optimistic (one author wrote its lexicons and the set): it is chosen as the least-bad front that costs no model time and behaves the same every time, not because it passes (D1.3) | W2; W7 | release gate |
| Front admission per instance | the qualified profile's `max_calls_per_instance`: ARM CPU 0 (1 only where the x86 run measures act p95 within 200 ms at one call); shared-GPU floor 4 for LFM2.5-1.2B and LFM2.5-2.6B, 2 for Qwen3.5-2B; the pool's capacity test sets the real value. On the measured shared GPU only LFM2.5-1.2B (70 ms) meets the single-call act row; LFM2.5-2.6B (144 ms) and Qwen3.5-2B (112 to 162 ms) qualify only on a device class that measures 100 ms or less (D1.4) | W2 | `front_model.admission_permits` |
| Front context rebase | at 6,144 tokens, keeping 8 decisions (history grows 100 to 170 tokens per decision) | W2 | `front_model.max_context_tokens`, `rebase_keep_decisions` |
| Eager end of turn | Smart Turn p ≥ 0.8; withdrawn after 300 ms voiced; at most 2 per turn | W5 | `duplex.endpoint.eager.*` |
| Task limits; cancel confirmation; reconnect grace | 3 active, 2 queued, 50 per session; 10 s; 60 s after an abnormal disconnect only (a clean close skips it) | W3 | `tasks.limits.*`; `tasks.cancel_confirm_timeout_ms`; `tasks.reconnect_grace_s` |
| Shutdown (Addendum D6.3) | no new task or provisional run after the shutdown signal; in-flight side-effecting tool calls finish within the drain window (30 s on the TLS path); then still-live tasks `unknown` | W3; W1 | the gateway's own drain (`main.rs:537-538`); no interaction key |
| Caller memory | off; 3 facts read within 150 ms; 10 stored; 90 days | W2 | `front_model.memory.*` |
| Tenant personas | off; 20 per tenant, 5 versions each | W2 | `front_model.tenant_personas.*` |

Experience targets are W7's (chapter 7). Addendum C6 changes three: in fast mode the first sound is the answer, held
to time to answer (Release 0 baseline + 150 ms at p50) and to a first sound by 1.6 s p50 from the acoustic end; the
generated-acknowledgement target is re-derived at the Release 0 exit on dedicated front hardware (measured first
spoken chunk on a shared GPU: LFM2.5-1.2B 575 to 656 ms p95), and that row is not a front qualification gate; barge-in
is split into "yields" (duck audible, p50 at most 0.3 s from onset) and "silent" (p50 at most 0.7 s at a `/ws` ear).
Addendum D7 splits the clip target into a gateway-owned part measured from E (at most `ack_due_ms` + transport lead +
playout + jitter) and a per-stack number from the acoustic end, set at the Release 0 exit from the measured median E;
early-turn time to answer is reported apart from later turns. Addendum D5 limits the `/ws` agent surface's
time-to-answer gate to Bud agents with a declared answer marker.

## 10. Configuration paths

Session homes: the `/ws` envelope, `RealtimeSessionConfig`, `RealtimeSettings`, `AliasDefinition`, `VoiceAgentEntry`,
a DAG definition, server `interaction.defaults.agent` (R1) and `interaction.defaults.realtime` (R3). A server key sent
by a session gets `interaction_override_refused`.

| Path | Home | Owner | Default | From |
|---|---|---|---|---|
| `interaction` (`true` or object); `.regime`, `.filler_owner`, `.backchannel_owner`, `.idle_owner` | session | W6 | absent = off; `auto` | R1 (A), R3 |
| `interaction.speech.*` (`voice_owner`, `confirmations`, `voice_mismatch`, `truth`, `claim_policy`, `narrator`, `clip_voice`; `realtime` on `/ws` R4, the only route for Bud deployments to Inworld, Ultravox and Yandex, sign-off 27) | session | W6 | `truth: monitored`, `narrator: templates` | R3 |
| `interaction.hitl.channels`, `interaction.events.*` | session | W6 | `[voice, client]` (SIP `[voice]`) | R1 |
| `interaction.runtime.*` (with `time.*`, `idle.*`) | session | W1 | W1 3.13 | R1 |
| `interaction.front.profile`, `persona`, `say_mode`, `act_budget_ms`, `total_budget_ms`, `max_words`, `backchannel_via_model` | session | W2 | `auto` (a name in `front_model.profiles_enabled`), default persona, `compose`, 300, 1,500, 15, `false` | R1 |
| `interaction.front.caller.preferred_name`, `verbosity`, `pace`, `language` (the four client-settable fields) | session | W2 | none | R1 |
| `interaction.front.caller.identity_token` (operator-signed, not a caller field; sign-off 16) | session | W2 (format), W6 (carries) | none; invalid gives `caller_identity_rejected` | R2 |
| `interaction.tasks.*` (`default_backend`, `backends` by name, `speculative_start`, `answer_stream`, `limits`) | session | W3 | brain; `auto` (on for the LLM tier; `on` refused on an unsafe backend); `auto` | R1 |
| `interaction.policy.*` | session | W4 | W4 3.11; echoed as `policy_hash` and the effective values | R1 |
| `interaction.duplex.barge_in` (with `duck_on_bare_onset`; `segmented.*` takes effect in R2, Addendum D2), `echo`, `endpoint`, `overlap_context`; `backchannel` (R2) | session | W5 | W5 3.11; echoed barge-in values | R1 |
| server `interaction.enabled` (env `WAAV_INTERACTION_ENABLED`: the canary-pod rollout switch and the rollback, Addendum D6.3); the key is read from the same YAML file as `ServerConfig`, or from `WAAV_INTERACTION_*` when `ServerConfig` came from the environment (`main.rs:155-160` picks file or environment) | server | W1 | `false` | R1 (checked in R0) |
| server `interaction.defaults.agent` (R1) with `defaults.agent_ids` (empty = every agent; the pilot-agent, then agent-by-agent rollout list, D6.3), `defaults.realtime` (R3), `speech.truth_ack`, `speech.fallback` (R3), `allow_client_override` (R2), `realtime_legacy_wire` | server | W6 | none or `[]` | R1 |
| server `interaction.tasks.task_backends`, `allow_client_backends`, `allow_client_tools`, `allow_client_push`, `phone_continuation`, `speech_policy`, task Redis | server | W3 | W3 3.13 | R1 |
| server `interaction.duplex.compute.*` | server only, refused from sessions | W5 | half the cores; 64 | R1 |
| server `front_model.*` (`enabled`, `mode`, `licence`, `profile`, `profiles_enabled`, `allow_unqualified`, `admission_permits`, `max_context_tokens`, `pool.*` R2) | separate key | W2 | `auto`; licence unset; `allow_unqualified: false` | R0 dark, R1 |
| server `front_model.memory.*` (`enabled` false, `identity_key_file`, `retention_days` 90, `read_budget_ms` 150, `max_facts_read` 3, `max_facts_stored` 10); `front_model.tenant_personas.*` (`enabled` false, `max_per_tenant` 20, `max_versions` 5) | separate key | W2 | off | R2 |
| budapp `VoiceAgentEntry.background.answer_marker` (requested from budapp for 2026-11-02) | agent definition | budapp (cross-team) | unset: the answer is relayed whole at completion | R1 |
| Operator route for the same marker (Addendum D5; W3 3.13 and 3.17.2): server `interaction.defaults.agent.background.answer_marker` for the agents in `defaults.agent_ids`, or `background.answer_marker` in the `interaction` value of an interaction-only alias (`AliasDefinition`, server configuration only, `core/alias/mod.rs:145-171`). Only for an agent the operator knows emits the marker; budapp's value wins when present; never client-settable | server | W3 (meaning), W6 (carriage) | unset | R1 |

Removed: `interaction.persona`, `persona.preferences`, `runtime.answer_window_ms`, `runtime.duck_on_bare_onset`,
`duplex.barge_in.answer_window_ms`, `front.breaker.*`, `front_deadline_ms`, `truth_guard_ack`, `licence_ack`,
`interaction.duplex_compute`, session `duplex.compute`, top-level `interaction.task_backends` (and its siblings),
`interaction.memory.*`, `interaction.personas_api.*`, `interaction.front.caller_ref`, `interaction.front.caller.name`.

## 11. Client events, the task-event mapping and codes

Event names, bodies and protocol 1.1 are W6's (`providers/wire_events.md`; `bud.*` on `/v1/realtime`). Each W3 event
maps as follows (W3 section 3.4.2; Addendum C4); every task event carries `state` and `cancelling`.

| W3 `TaskEventKind` | Client event |
|---|---|
| `Created` (at adoption only; a provisional run emits nothing) | `task_created` (`speculative: true` when adopted) |
| `Working`, `Queued`, `StillQueued`, `Progress`, `Retried`, `Resumed`, `Revised`, `InputAdded`, `Superseded`, `EffectsUpdated`, `Orphaned`, `DeadlineReached`, `AnswerStopped` | `task_progress`; `answer_stopped` carries `{reason, released_sentences}` |
| `CancelRequested`, `CancelUnconfirmed` | `task_progress` with `cancelling: true` and the state unchanged; never `task_cancelled` or `unknown` |
| `InputRequired`, `AuthRequired`, `CancelNeedsConfirmation` | `task_question` (W4 renders the words) |
| `InputResolved` | `task_answer_received` |
| `AnswerSentence` | no task event; one `assistant_transcript` (`source: background`, `verbatim: true`) per sentence as it plays |
| `Completed`, `Failed`, `Rejected`, `Unknown`, `StaleOutcome` | `task_result`, with `effects` |
| `Cancelled` | `task_cancelled`, with `effects` |

`config_warning` codes are W6's list (`providers/wire_events.md`, Warning codes). Addendum C4 fixes these names in it:
`interaction_missing_build_features` (no `silero-vad` and `smart-turn` features, `gateway/Cargo.toml:96,106`),
`interaction_language_step_down`, `interaction_surface_unavailable`, `caller_memory_unavailable`,
`model_family_unknown`, `regime_downgraded`, `truth_ack_required`, `interaction_dag_audio_output`,
`front_model_scripted`, `caller_identity_rejected`, `speculative_start_unsafe_backend`. `floor_state.reason` gains
`stream_gap`. The optional `code` on the `/ws` `error` message (Release 1) carries `question_closed` (Addendum A2
item 9) and W1's `interaction_failed`. Language step-down happens only on `/ws` conversation and agent sessions; raw
`/ws`, the GA cascade and regimes B and C play a repeat-request template and continue in English (Addendum C7).

## 12. Golden tests (Release 0)

`runtime_contract_golden` (W1), `front_act_contract.json` (W2, the model), `task_contract_golden_file_matches` (W3),
`policy_contract_golden` (W4), `duplex_contract_golden` (W5), `config_paths_match_contract_golden` and
`wire_names_match_contract_golden` (W6): each fails when one side changes a shared type alone.

## 13. Aligned with Addenda A to D

What changed: the deadline anchored at E and the eager-start rule (A1); half-cascade owners, regime narrowing, the
Grok and Azure model fields, server task, compute and caller-memory paths, the `/ws` error `code` (A2); `scripted` in
Release 1, `lfm-large`, the Apache candidates, admission, rebase and `POST /completion` (B); `AnswerSentence`,
`StreamSpan`, the marker rule and Bud streaming only with `background.answer_marker` (C1); the provisional-run API and
unsafe-backend refusal (C2); `EndOfTurnReport` (C3); the names, codes, task-event mapping, echo and `PolicyConfig`
(C4); the identity token and `preferred_name` (C5); experience targets (C6); session behaviour (C7). Addendum D: the
router's question step, its recall gate and the full front qualification gates (D1, sections 4 and 9); segmented
sessions moved to Release 2 (D2, section 7); the release target windows (D3, "Reading the code"); the operator and
alias route for `answer_marker` and the agent-surface time-to-answer gate (D5, sections 5 and 10); shutdown and the
rollout levers (D6.3, sections 5, 9 and 10); the split clip target (D7, section 9).

Withdrawn names (designs still using them are corrected by their own aligners): `AnswerChunk` -> `AnswerSentence`;
W4's `index`/`chunks` -> `seq`/`sentences`, `ToolRound` -> `MarkerThenToolCall`; W1's `ProvisionalId` ->
`ProvisionalKey`, `CancelWhy` -> `DiscardReason`, `matches_eager` -> `EndOfTurnReport.eager`; W3's
`ProvisionalRequest` -> `start_provisional(origin, words, adopt_by)`; W5's `by` -> `source`, `EagerMatch::Differs`
-> `Different`, `Withdrawn` removed; `on_vad_edge` -> `VadOnset` from `VoiceOnset`; W6's `SpeechPurpose` and
`Confirmation { irreversible }` -> `SpeakPurpose::Confirmation`; W1's `interaction_build_unsupported` and
`interaction_stepped_down` -> `interaction_missing_build_features` and `interaction_language_step_down`; W2's
`caller_ref` and `caller_ref_invalid` -> `identity_token` and `caller_identity_rejected`; a Bud agent's speak field as
a streaming route (W3 3.17.5) -> the declared marker only; the preamble lexicon (W7) -> the `ANSWER:` marker rule; in
`providers/wire_events.md`, `answer_stopped.released_chunks` -> `released_sentences` and an unconfirmed cancel as
`task_cancelled` with `state: unknown` -> `task_progress` with `cancelling: true`.

## 14. Still unsettled

1. **A declined provisional start.** Addendum C2 writes `start_provisional(...) -> ProvisionalKey`; W3 3.16.1 returns
   `None` when speculation is off for the backend or no slot or permit is free. Read here as: the key is always
   returned, no run exists behind it, `delegate` with it starts a normal run, and W1 records `Off` or `Refused`.
2. **Type names for the addendum's arguments.** `ProvisionalOrigin` and `ProvisionalWords` are this file's names, and
   `FrontAtE` and `EagerEndOfTurn` carry the turn so the key can be formed. W3 to confirm in its section 3.16.
3. **Irreversible confirmations on provider voices.** With `irreversible` off the tag, W6 must read it from W4's
   `HitlRequest` (or keep every confirmation off the guided speech path).
4. **Codes outside the list:** W3's `interaction_dag_no_text_output` (Release 3); `Partial` has no client mapping.
5. **Identity token format:** settled by Addendum E3: W2's `v1.<payload>.<mac>`; the schema's pattern already matches.
6. **Smaller names:** `HeardHistory` (W1) or `ConversationSnapshot` (W3); `HeardReport` is not yet defined;
   `OverlapNote.task` typed `TaskId` where W5 writes `task_ref`; `TimelineMs` as a newtype is this file's choice.
7. **`lfm-large` admission:** the 4-call floor is measured (Addendum B7) but not yet in W2's qualification table.
8. **Shutdown names and bound (Addendum D6.3).** `SessionEndReason::ServerShutdown` and `DelegateRefusal::ShuttingDown`
   are this file's names, for W1 and W3 to confirm. The plain-HTTP path has no drain bound in code (`main.rs:558-563`),
   so W3 needs a stated deadline, inside the pod's termination grace period, at which it records still-live tasks
   `unknown`.
9. **Where the operator's `answer_marker` sits (Addendum D5).** W3 writes `interaction.defaults.agent.background`, but
   the published schema's interaction value has no `background` block yet (`providers/interaction_config.schema.json`,
   `ServerSideInteractionSetting`); W6 adds it to the server-side homes only (server defaults and aliases, never a
   client).
10. **A segmented session in Release 1:** settled by Addendum E4: interaction off with `config_warning`
    `interaction_surface_unavailable` (`{surface, release: 2}`).
11. **Provider-call provisional backstop:** settled by Addendum E2 (`adopt_by` + 500 ms, per-provider `adopt_by`).

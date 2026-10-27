//! The floor machine: who holds the floor, the lane executor (playing, one synthesis in
//! flight, pending, a parked relay) and the heard timeline.
//!
//! What the actor does with it in one loop turn:
//! - every pump event goes to [`FloorMachine::on_pump`], every duplex action to
//!   [`FloorMachine::on_duplex`];
//! - [`FloorMachine::on_settled`] for each `PolicyAction::Settled` and
//!   [`FloorMachine::on_board`] with the registry's board (the two interlocks the floor
//!   cannot see from audio; without them the backstops apply);
//! - [`FloorMachine::offer`] for each `PolicyAction::Speak`, [`FloorMachine::drop_item`] for
//!   each `PolicyAction::Drop`;
//! - [`FloorMachine::next_cmds`]: at most one `Submit`. A `Cut` is performed by the actor
//!   (`PumpControl::cut`, then `SpeechCmd::Cut`) and reported with [`FloorMachine::on_cut`];
//!   a duplex cut takes the same road, so the actor has one cut routine;
//! - [`FloorMachine::tick`] on a wake-up: the timers whose result is a lane event.
//!
//! Warm-up utterances (`discard`) never produce a lane event.

use std::collections::VecDeque;
use std::sync::Arc;

use super::config::InteractionConfig;
use super::guard;
use super::persona::Persona;
use super::policy::GOING_ON_MS;
use super::types::{
    Board, CutCause, DuplexAction, Floor, FloorCmd, FloorSnapshot, GateCmd, ItemId, Kind, Lane,
    LaneEvent, Ms, PumpEvent, Say, TurnId, UttId, Utterance,
};

/// The client's output lag: a frame is heard this long after its playout time (the pump's
/// lead after it was delivered). A socket client and a phone line: 150 ms; a room's WebRTC
/// participant (a pump lead under the socket's): 80 ms.
const LAG_MS: Ms = 150;
const ROOM_LAG_MS: Ms = 80;
/// No audio and no completion this long after the last progress: the synthesis has failed.
const WATCHDOG_MS: Ms = 10_000;
/// How long a relay keeps the floor while its next sentence is late.
const STREAM_GAP_MS: Ms = 600;
/// Backstops: the turn decision after E, an undecided yield, and thinking beyond the
/// holding line.
const TURN_DECISION_MS: Ms = 500;
const YIELD_MS: Ms = 7_700;
const THINKING_EXTRA_MS: Ms = 2_000;
/// Ranks up to the relay's keep the floor through a stream gap.
const RELAY_RANK: u8 = 4;
/// A question with a deadline that cannot wait for a slot (chapter 8): it cuts a relayed
/// answer at its next clause boundary, and it may start in a pause of the caller's.
pub const RANK_URGENT: u8 = 1;
/// The longest wait for a clause boundary before such a cut; then the next word end.
const PREEMPT_MAX_MS: Ms = 400;
/// A caller's pause this long lets an urgent question start.
const URGENT_PAUSE_MS: Ms = 250;
/// Agent lines kept for the echo filter.
const LINES_KEPT: usize = 8;
/// A bank line that shares a phrase with a line the agent said this recently is passed over
/// for another of its bank ([`FloorMachine::fresh`]): "a few seconds", longer than the
/// holding line's silence (`hold_after_silence_ms`, 8 s).
const SAID_LATELY_MS: Ms = 10_000;
/// A relayed sentence that an urgent question cut once the caller had heard this share of
/// its characters is not said again after the question ([`FloorMachine::finish_cut`]). Just
/// under half: the heard estimate ends at the last whole word before the playout position
/// less the pump's lead, so it trails what was played. Live (take 15) 3.9 s of a 6.9 s
/// sentence had played, about 61 of its 123 characters by that estimate.
const MOSTLY_HEARD_PERCENT: usize = 45;
/// What the agent says to "say that again" when the caller has heard nothing yet.
const NOTHING_TO_REPEAT: Say = Say::Bank {
    group: "clarify",
    intent: None,
};

/// An utterance handed to the provider and not yet heard.
struct Active {
    utt: UttId,
    utterance: Utterance,
    text: String,
    /// The last progress (submit, head of the queue, ran dry): the watchdog counts from here.
    since: Ms,
    started: bool,
    synth_done: bool,
    /// The pump ran out of its audio before the synthesis finished.
    ran_dry: Option<Ms>,
}

/// An urgent line rendered ahead of its cut ([`FloorCmd::Prerender`]).
struct Prerender {
    utterance: Utterance,
    utt: UttId,
    text: String,
    since: Ms,
    done: bool,
}

impl Active {
    /// Its audio is reaching the caller.
    fn audible(&self) -> bool {
        self.started && self.ran_dry.is_none()
    }
}

pub struct FloorMachine {
    persona: Arc<Persona>,
    /// The pump's playout lead (`PumpControl::lead_ms`).
    lead_ms: Ms,
    /// The client's output lag after the playout time.
    lag_ms: Ms,
    thinking_ms: Ms,
    /// The latest `now` seen.
    clock: Ms,
    next_utt: u64,
    /// Oldest first; only the last one may still be in synthesis.
    active: VecDeque<Active>,
    /// By rank, then by arrival.
    pending: Vec<Utterance>,
    /// A relay keeping the floor until its next sentence or this time.
    gap: Option<(ItemId, Lane, Ms)>,
    parked: Option<ItemId>,
    user_speaking: bool,
    yield_until: Option<Ms>,
    /// The latest turn and the backstop of its decision.
    turn_until: Option<(TurnId, Ms)>,
    think_until: Option<Ms>,
    question_open: bool,
    task_live: bool,
    /// A cut the actor must perform; `true` once it was handed over.
    cut: Option<(CutCause, bool)>,
    lines: VecDeque<(Ms, String)>,
    /// The caller cut the last of `lines` by speaking over it.
    last_interrupted: bool,
    /// The last line the caller heard in full: what `Say::Repeat` says again.
    last_heard: Option<String>,
    /// The line the client last reported playing (`playback_position`): when set, what
    /// `Say::Repeat` says again.
    repeat: Option<String>,
    /// An urgent question waits: when the relay that plays is cut for it, and that the cut
    /// hands the relay's sentences back to the queue instead of dropping them.
    preempt: Option<Ms>,
    requeue: bool,
    /// The urgent lines synthesised into the speech cache while the relayed answer they break
    /// into still plays, in queue order: each said from the cut with these words.
    prerender: Vec<Prerender>,
    /// A cut for a caller who went on with their turn: the words owed to them go back too.
    taken_back: bool,
    /// When the caller's latest turn ended (E).
    ended_at: Option<Ms>,
    /// The speaking rate the actor measured, for where a clause ends in the playing line.
    chars_per_ms: f64,
    /// The caller is in a pause of a turn since then (the analyser's offset, back-dated to
    /// the last voiced audio); `None` while they make a sound.
    paused_since: Option<Ms>,
    /// A listening sound was wanted (the duplex machine's `Backchannel`): from then on one
    /// still to be made is made while the caller speaks.
    clips_wanted: bool,
    out: Vec<LaneEvent>,
}

impl FloorMachine {
    /// The persona resolves an utterance's words for the heard timeline, with the `UttId`
    /// as the rotation index. `lead_ms` is the pump's playout lead.
    pub fn new(cfg: &InteractionConfig, persona: Arc<Persona>, lead_ms: Ms) -> Self {
        FloorMachine {
            persona,
            lead_ms,
            lag_ms: if lead_ms < LAG_MS {
                ROOM_LAG_MS
            } else {
                LAG_MS
            },
            thinking_ms: Ms::from(cfg.policy.hold_after_silence_ms) + THINKING_EXTRA_MS,
            clock: 0,
            next_utt: 1,
            active: VecDeque::new(),
            pending: Vec::new(),
            gap: None,
            parked: None,
            user_speaking: false,
            yield_until: None,
            turn_until: None,
            think_until: None,
            question_open: false,
            task_live: false,
            cut: None,
            lines: VecDeque::new(),
            last_interrupted: false,
            last_heard: None,
            repeat: None,
            preempt: None,
            requeue: false,
            prerender: Vec::new(),
            taken_back: false,
            ended_at: None,
            chars_per_ms: 0.015,
            paused_since: None,
            clips_wanted: false,
            out: Vec::new(),
        }
    }

    /// The speaking rate as last measured (characters per millisecond of audio).
    pub fn set_speech_rate(&mut self, chars_per_ms: f64) {
        if chars_per_ms > 0.0 {
            self.chars_per_ms = chars_per_ms;
        }
    }

    /// The caller's voice: `false` at the analyser's offset (`since`: their last voiced audio),
    /// `true` at an onset.
    pub fn on_voice(&mut self, voiced: bool, since: Ms) {
        self.paused_since = (!voiced).then_some(since);
    }

    /// The characters of the agent's speech still to be heard: queued, with the provider or
    /// playing (warm-ups aside). With the speaking rate, how long until the agent is done.
    pub fn chars_left(&self) -> usize {
        let queued = self.pending.iter().filter(|queued| !queued.discard);
        let queued = queued.filter_map(|queued| self.persona.resolve(&queued.say, 0));
        let active = self
            .active
            .iter()
            .filter(|active| !active.utterance.discard);
        queued
            .map(|text| text.chars().count())
            .chain(active.map(|active| active.text.chars().count()))
            .sum()
    }

    /// The relayed answers queued, with the provider or playing, by item.
    pub fn relay_items(&self) -> Vec<ItemId> {
        let active = self.active.iter().map(|active| &active.utterance);
        let mut items: Vec<ItemId> = active
            .chain(&self.pending)
            .filter(|utterance| utterance.kind == Kind::Relay)
            .map(|utterance| utterance.item)
            .collect();
        items.dedup();
        items
    }

    /// The room's caller is on a phone line: heard as late as on a socket.
    pub fn set_phone_line(&mut self) {
        self.lag_ms = LAG_MS;
    }

    /// The client reported playing this line: "say that again" says it, whatever the pump's
    /// estimate of what was heard.
    pub fn set_repeat(&mut self, line: String) {
        self.repeat = Some(line);
    }

    /// The policy settings after an `interaction_update`: the thinking backstop follows
    /// `hold_after_silence_ms` from the next time it is armed.
    pub fn set_hold_after_silence(&mut self, ms: u32) {
        self.thinking_ms = Ms::from(ms) + THINKING_EXTRA_MS;
    }

    /// A cut waits for the actor to perform it.
    pub fn cut_pending(&self) -> bool {
        self.cut.is_some()
    }

    pub fn on_pump(&mut self, now: Ms, event: PumpEvent) -> Vec<LaneEvent> {
        self.clock = now;
        if let PumpEvent::SynthDone { utt } | PumpEvent::Failed { utt } = event
            && let Some(at) = self.prerender.iter().position(|ahead| ahead.utt == utt)
        {
            // A line rendered ahead: in the speech cache, or said afresh at its cut.
            match event {
                PumpEvent::SynthDone { .. } => self.prerender[at].done = true,
                _ => drop(self.prerender.remove(at)),
            }
            return std::mem::take(&mut self.out);
        }
        match event {
            PumpEvent::Started { utt, at } => {
                if let Some(index) = self.index(utt) {
                    // The pump plays in order: everything ahead of this one has ended.
                    self.heard_through(index, at);
                    if let Some(head) = self.active.front_mut() {
                        head.ran_dry = None;
                        head.since = at;
                        let item = head.utterance.item;
                        if !std::mem::replace(&mut head.started, true) {
                            self.out.push(LaneEvent::Started { item, utt, at });
                        }
                        // It speaks, for the first time or again after a stream gap.
                        self.resume(item);
                    }
                }
            }
            PumpEvent::SynthDone { utt } => {
                if let Some(index) = self.index(utt) {
                    let active = &mut self.active[index];
                    active.synth_done = true;
                    let (discard, ran_dry) = (active.utterance.discard, active.ran_dry);
                    if discard {
                        self.active.remove(index);
                    } else if let Some(at) = ran_dry {
                        // Everything it had was already played.
                        self.heard_through(index + 1, at);
                        self.lanes_idle(at);
                    }
                }
            }
            PumpEvent::Idle { utt, at, .. } => {
                if let Some(index) = self.index(utt) {
                    self.heard_through(index, at);
                    let head = &mut self.active[0];
                    if !head.started {
                        // The provider finished without a frame of audio.
                        self.fail(0, now);
                    } else if head.synth_done {
                        self.heard_through(1, at);
                        self.lanes_idle(at);
                    } else {
                        // Stream gap: the audio ran out before the synthesis did.
                        head.ran_dry = Some(at);
                        head.since = at;
                        let (item, lane) = (head.utterance.item, head.utterance.lane);
                        self.gap = Some((item, lane, at + STREAM_GAP_MS));
                    }
                }
            }
            PumpEvent::Failed { utt } => {
                if let Some(index) = self.index(utt) {
                    self.fail(index, now);
                }
            }
            PumpEvent::FirstDuckedFrame { .. } => {}
        }
        std::mem::take(&mut self.out)
    }

    pub fn on_duplex(&mut self, now: Ms, action: &DuplexAction) -> Vec<LaneEvent> {
        self.clock = now;
        match action {
            DuplexAction::UserSpeaking(on) => {
                self.user_speaking = *on;
                if *on {
                    self.take_back_unheard(now);
                }
            }
            DuplexAction::Gate(GateCmd::Duck | GateCmd::Pause) => {
                if self.yield_until.is_none() && self.active.iter().any(Active::audible) {
                    self.yield_until = Some(now + YIELD_MS);
                }
            }
            DuplexAction::Gate(GateCmd::Resume | GateCmd::Open) => self.yield_until = None,
            DuplexAction::Cut(cause) => self.want_cut(now, *cause),
            DuplexAction::EndOfTurn(turn) => {
                self.user_speaking = false;
                self.ended_at = Some(turn.at);
                self.turn_until = Some((turn.turn, turn.at + TURN_DECISION_MS));
            }
            DuplexAction::Backchannel { .. } => self.clips_wanted = true,
            DuplexAction::Classified(_)
            | DuplexAction::Eager { .. }
            | DuplexAction::EagerWithdrawn { .. }
            | DuplexAction::Interim { .. }
            | DuplexAction::ClaimSegment
            | DuplexAction::FinalizeStt { .. } => {}
        }
        std::mem::take(&mut self.out)
    }

    /// The policy settled this turn: ranks 3 to 8 no longer wait for its decision.
    pub fn on_settled(&mut self, now: Ms, turn: TurnId) {
        self.clock = now;
        if self.turn_until.is_some_and(|(pending, _)| pending == turn) {
            self.turn_until = None;
        }
    }

    /// The registry's view: whether a question is open and whether a task runs.
    pub fn on_board(&mut self, now: Ms, board: &Board) {
        self.clock = now;
        let live = board.tasks.iter().any(|task| task.state.is_working());
        let started = live && !self.task_live;
        self.task_live = live;
        self.question_open = board.open_question.is_some();
        if started {
            self.think_until = Some(now + self.thinking_ms);
        } else if !live {
            self.rest_unless_owed(now);
        }
    }

    /// Queues an utterance. An utterance with empty verbatim words is never spoken: with
    /// `last` it closes its item's relay (the terminal task event arrives after the last
    /// sentence was offered).
    pub fn offer(&mut self, now: Ms, utterance: Utterance) -> Vec<LaneEvent> {
        self.clock = now;
        if matches!(&utterance.say, Say::Verbatim(text) if text.is_empty()) {
            if utterance.last {
                self.close_relay(utterance.item);
            }
        } else {
            if !utterance.discard {
                self.think_until = Some(now + self.thinking_ms);
            }
            let at = self
                .pending
                .iter()
                .position(|queued| queued.rank > utterance.rank)
                .unwrap_or(self.pending.len());
            self.pending.insert(at, utterance);
        }
        std::mem::take(&mut self.out)
    }

    /// Takes an item back: its queued utterances are `Returned`; if one is with the provider
    /// or playing, the speech is cut (`FloorCmd::Cut(PreEmpted)`).
    pub fn drop_item(&mut self, now: Ms, item: ItemId) -> Vec<LaneEvent> {
        self.clock = now;
        let (dropped, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|queued| queued.item == item);
        self.pending = kept;
        self.give_back(dropped);
        self.resume(item);
        if self.active.iter().any(|a| a.utterance.item == item) {
            self.want_cut(now, CutCause::PreEmpted);
        }
        std::mem::take(&mut self.out)
    }

    /// The actor cut the speech: `heard` holds the characters delivered per utterance that
    /// was in the pump. Playing utterances are `Dropped`, the others `Returned`; a cut by
    /// the caller also returns everything that was queued.
    pub fn on_cut(&mut self, now: Ms, cause: CutCause, heard: &[(UttId, usize)]) -> Vec<LaneEvent> {
        self.clock = now;
        self.finish_cut(now, cause, heard, true);
        std::mem::take(&mut self.out)
    }

    /// The timers: expiry, the synthesis watchdog, the stream gap and the backstops.
    pub fn tick(&mut self, now: Ms) -> Vec<LaneEvent> {
        self.timers(now);
        std::mem::take(&mut self.out)
    }

    /// What the actor must do now; at most one `Submit`, with the words the utterance
    /// resolved to (empty when it has none).
    pub fn next_cmds(&mut self, now: Ms) -> Vec<FloorCmd> {
        self.timers(now);
        if let Some((cause, handed)) = &mut self.cut {
            // Nothing is submitted until the cut is reported (or the next timer run).
            *handed = true;
            return vec![FloorCmd::Cut(*cause)];
        }
        let mut cmds = Vec::new();
        // A warm-up in synthesis gives way to a line owed to the caller (live L1: an
        // acknowledgement waited 1.2 to 2.3 s for a failure line's render). Nothing of a
        // warm-up plays, so its synthesis is dropped and it waits in the queue again.
        let warming = |active: &Active| active.utterance.discard && !active.synth_done;
        if self.active.iter().all(|active| active.utterance.discard)
            && self.active.iter().any(warming)
            && self.pending.iter().any(|queued| !queued.discard)
        {
            let (dropped, kept): (VecDeque<Active>, VecDeque<Active>) =
                std::mem::take(&mut self.active)
                    .into_iter()
                    .partition(warming);
            self.active = kept;
            if self
                .startable(now)
                .is_some_and(|at| !self.pending[at].discard)
            {
                cmds.push(FloorCmd::DropWarm(dropped.iter().map(|a| a.utt).collect()));
                self.pending
                    .extend(dropped.into_iter().map(|active| active.utterance));
            } else {
                self.active.extend(dropped);
            }
        }
        let Some(index) = self.startable(now) else {
            cmds.extend(self.prerender_next(now));
            return cmds;
        };
        let utterance = self.pending.remove(index);
        let rendered =
            (self.prerender.iter()).position(|ahead| ahead.done && ahead.utterance == utterance);
        if let Some(ahead) = rendered.map(|at| self.prerender.remove(at)) {
            // Its words are in the speech cache: the same words, as the same utterance.
            cmds.push(FloorCmd::Submit {
                utt: ahead.utt,
                utterance: utterance.clone(),
                text: ahead.text.clone(),
            });
            self.active.push_back(Active {
                utt: ahead.utt,
                utterance,
                text: ahead.text,
                since: now,
                started: false,
                synth_done: false,
                ran_dry: None,
            });
            return cmds;
        }
        let utt = UttId(self.next_utt);
        self.next_utt += 1;
        let said = match &utterance.say {
            Say::Repeat => self
                .repeat
                .clone()
                .or_else(|| self.last_heard.clone())
                .or_else(|| self.persona.resolve(&NOTHING_TO_REPEAT, utt.0)),
            say => self.fresh(now, say, utt.0),
        };
        let text = said.unwrap_or_default();
        let cmd = FloorCmd::Submit {
            utt,
            utterance: utterance.clone(),
            text: text.clone(),
        };
        self.active.push_back(Active {
            utt,
            utterance,
            text,
            since: now,
            started: false,
            synth_done: false,
            ran_dry: None,
        });
        cmds.push(cmd);
        cmds
    }

    /// Nothing of the agent's is with the provider, playing or queued, and no relay holds the
    /// floor for its next sentence: a task's update may be said (chapter 8's slot).
    /// Whether the caller is speaking (the analyser's last word on it).
    pub fn user_speaking(&self) -> bool {
        self.user_speaking
    }

    pub fn quiet(&self) -> bool {
        self.nothing_queued() && self.gap.is_none_or(|gap| self.clock >= gap.2)
    }

    /// Since when the line with the provider has given no audio, when one waits for its
    /// first (a warm-up or a clip in synthesis aside): the health watcher's synthesis stall
    /// (chapter 10), long before the watchdog fails the line.
    pub fn synth_waiting_since(&self) -> Option<(UttId, Ms)> {
        let head = self.active.front()?;
        let waits = !head.started && !head.synth_done && !head.utterance.discard;
        waits.then_some((head.utt, head.since))
    }

    /// The last line the caller heard: the one the client reported playing, else the last
    /// heard in full.
    pub fn last_heard(&self) -> Option<&str> {
        self.repeat.as_deref().or(self.last_heard.as_deref())
    }

    /// When the agent's last line ended (played through, cut or failed).
    pub fn last_line_at(&self) -> Option<Ms> {
        self.lines.back().map(|(at, _)| *at)
    }

    /// When the agent went quiet with the caller listening: the end of its last line, unless
    /// the caller cut that line by speaking (then the caller was the one talking).
    pub fn quiet_since(&self) -> Option<Ms> {
        self.last_line_at().filter(|_| !self.last_interrupted)
    }

    /// The agent's recent lines, each with when it ended; a line still playing ends "now".
    pub fn recent_agent_lines(&self) -> Vec<(Ms, String)> {
        let mut recent_agent_lines: Vec<(Ms, String)> = self.lines.iter().cloned().collect();
        recent_agent_lines.extend(
            self.active
                .iter()
                .filter(|active| active.started)
                .map(|active| (self.clock, active.text.clone())),
        );
        recent_agent_lines
    }

    /// A line the agent said apart from the floor (a failure line the pump played, chapter
    /// 10), which ended at `at`: it joins the heard timeline, so a line said next does not
    /// repeat its words ([`FloorMachine::fresh`]) and the echo filter knows it.
    pub fn said_apart(&mut self, at: Ms, line: String) {
        self.clock = self.clock.max(at);
        self.remember(at, line, false);
    }

    pub fn snapshot(&self) -> FloorSnapshot {
        let recent_agent_lines = self.recent_agent_lines();
        FloorSnapshot {
            floor: self.floor(),
            agent_audible: self.active.iter().any(Active::audible),
            playing: self
                .active
                .iter()
                .find(|active| active.started)
                .map(|active| (active.utterance.item, active.utterance.kind)),
            recent_agent_lines,
        }
    }

    /// Holder, lane and reason of the `floor_state` event.
    pub fn wire_state(&self) -> (&'static str, Option<Lane>, &'static str) {
        let speaking = self.active.iter().find(|active| active.audible());
        let lane = speaking.map(|active| active.utterance.lane);
        match self.floor() {
            Floor::Listening => ("none", None, "idle"),
            Floor::Thinking if self.parked.is_some() => ("none", None, "stream_gap"),
            Floor::Thinking => ("assistant", None, "hold"),
            Floor::UserSpeaking if lane.is_some() => ("both", lane, "user_speech"),
            Floor::UserSpeaking => ("user", None, "user_speech"),
            Floor::Yielding => ("both", lane, "user_speech"),
            Floor::FrontSpeaking | Floor::MainSpeaking => {
                let reason = match speaking.map(|active| active.utterance.kind) {
                    Some(Kind::Hold | Kind::ChoiceOffer | Kind::Checkin | Kind::Progress) => "hold",
                    Some(Kind::Relay) | None => "relay",
                    Some(_) => "front_ack",
                };
                let lane = lane.or(self.gap.map(|(_, lane, _)| lane));
                ("assistant", lane, reason)
            }
        }
    }

    /// The next time `tick(now)` or `next_cmds(now)` has something to do.
    pub fn next_wakeup(&self) -> Option<Ms> {
        let handed_cut = matches!(self.cut, Some((_, true)));
        let submit = self.cut.is_some()
            || self.startable(self.clock).is_some()
            || self.prerender_wanted().is_some();
        // An urgent question may start once the caller's pause is long enough.
        let pause = self
            .paused_since
            .filter(|_| self.user_speaking && self.urgent_waits())
            .map(|since| since + URGENT_PAUSE_MS);
        let watchdog = self
            .active
            .front()
            .filter(|head| !head.audible())
            .map(|head| head.since + WATCHDOG_MS);
        let ahead = (self.prerender.iter())
            .filter(|ahead| !ahead.done)
            .map(|ahead| ahead.since + WATCHDOG_MS)
            .min();
        [
            ahead,
            (handed_cut || submit).then_some(self.clock),
            watchdog,
            self.gap.map(|(_, _, until)| until),
            self.yield_until,
            self.turn_until.map(|(_, until)| until),
            self.think_until,
            self.pending
                .iter()
                .filter_map(|queued| queued.expires_at)
                .min(),
            self.preempt,
            pause,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    fn floor(&self) -> Floor {
        let now = self.clock;
        if self.yield_until.is_some() {
            return Floor::Yielding;
        }
        if self.user_speaking {
            return Floor::UserSpeaking;
        }
        let lane = self
            .active
            .iter()
            .find(|active| active.audible())
            .map(|active| active.utterance.lane)
            .or(self.gap.filter(|gap| now < gap.2).map(|gap| gap.1));
        match lane {
            Some(Lane::Front) => Floor::FrontSpeaking,
            Some(Lane::Main) => Floor::MainSpeaking,
            None if self.active.iter().any(|active| !active.utterance.discard)
                || self.turn_until.is_some()
                || self.think_until.is_some() =>
            {
                Floor::Thinking
            }
            None => Floor::Listening,
        }
    }

    fn index(&self, utt: UttId) -> Option<usize> {
        self.active.iter().position(|active| active.utt == utt)
    }

    fn nothing_queued(&self) -> bool {
        self.active.iter().all(|active| active.utterance.discard)
            && self.pending.iter().all(|queued| queued.discard)
    }

    /// An urgent question is queued.
    fn urgent_waits(&self) -> bool {
        (self.pending.iter()).any(|queued| queued.rank <= RANK_URGENT && !queued.discard)
    }

    /// An urgent line is in synthesis ahead of its cut: the provider is busy with it.
    pub fn prerendering(&self) -> bool {
        self.prerender.iter().any(|ahead| !ahead.done)
    }

    /// The next urgent line to synthesise ahead of its cut: one waits for the cut of a relayed
    /// answer whose synthesis is done, and the provider has nothing else (live MA2: the
    /// question's synthesis, 0.55 to 0.9 s on Piper, began only at the cut, 0.06 to 0.41 s
    /// after the question came). With its words; a line without words is not wanted (else the
    /// floor would wake for it at once, again and again).
    fn prerender_wanted(&self) -> Option<(usize, String)> {
        let relay = |active: &Active| active.utterance.kind == Kind::Relay;
        let busy = self.prerendering()
            || self.cut.is_some()
            || self.active.iter().any(|active| !active.synth_done)
            || !self.active.iter().any(relay);
        if busy {
            return None;
        }
        let rendered =
            |queued: &Utterance| (self.prerender.iter()).any(|ahead| ahead.utterance == *queued);
        self.pending.iter().enumerate().find_map(|(at, queued)| {
            let wanted = queued.rank <= RANK_URGENT
                && !queued.discard
                && !matches!(queued.say, Say::Repeat)
                && !rendered(queued);
            let text = wanted.then(|| self.persona.resolve(&queued.say, self.next_utt));
            let text = text.flatten().filter(|text| !text.trim().is_empty())?;
            Some((at, text))
        })
    }

    fn prerender_next(&mut self, now: Ms) -> Option<FloorCmd> {
        let (at, text) = self.prerender_wanted()?;
        let utterance = self.pending[at].clone();
        let utt = UttId(self.next_utt);
        self.next_utt += 1;
        self.prerender.push(Prerender {
            utterance,
            utt,
            text: text.clone(),
            since: now,
            done: false,
        });
        Some(FloorCmd::Prerender { utt, text })
    }

    /// The first queued utterance that may be submitted now. While the caller speaks only an
    /// urgent question may start, in a pause of at least [`URGENT_PAUSE_MS`].
    fn startable(&self, now: Ms) -> Option<usize> {
        let paused = self
            .paused_since
            .is_some_and(|since| now >= since + URGENT_PAUSE_MS);
        if self.yield_until.is_some()
            || self.active.back().is_some_and(|last| !last.synth_done)
            || self.prerendering()
        {
            return None;
        }
        // An urgent question waits for the cut of the relayed answer it breaks into: handed to
        // the provider now, it would play after what the provider already has.
        let relay = |active: &Active| active.utterance.kind == Kind::Relay;
        if self.urgent_waits() && self.active.iter().any(relay) {
            return None;
        }
        if self.user_speaking {
            let urgent = |queued: &Utterance| queued.rank <= RANK_URGENT && !queued.discard;
            // A listening sound is wanted in this caller's long turn and is still to be made:
            // it is made in the turn (live BC2: the first session's bank waited behind the
            // warm-up lines, the TTS still cold, and then behind the caller's turn, and the
            // turn got none).
            let clip = |queued: &Utterance| self.clips_wanted && queued.kind == Kind::Clip;
            return (self.pending.iter().position(urgent).filter(|_| paused))
                .or_else(|| self.pending.iter().position(clip));
        }
        let turn_pending = self.turn_until.is_some_and(|(_, until)| now < until);
        let gap = self.gap.filter(|gap| now < gap.2);
        self.pending.iter().position(|queued| {
            if queued.discard {
                // A warm-up never competes with real speech.
                return self.floor() == Floor::Listening
                    && !self.task_live
                    && self.pending.iter().all(|other| other.discard);
            }
            let waits_for_turn = queued.rank >= 3 && turn_pending;
            let waits_for_answer =
                queued.rank >= 3 && self.question_open && queued.question.is_none();
            let waits_for_relay =
                gap.is_some_and(|gap| queued.item != gap.0 && queued.rank > RELAY_RANK);
            !(waits_for_turn || waits_for_answer || waits_for_relay)
        })
    }

    fn timers(&mut self, now: Ms) {
        self.clock = now;
        if let Some((cause, true)) = self.cut {
            // The actor did not report the cut: nothing was heard as far as we know.
            self.finish_cut(now, cause, &[], true);
        }
        let (expired, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|queued| queued.expires_at.is_some_and(|at| at <= now));
        self.pending = kept;
        self.give_back(expired);
        if self
            .active
            .front()
            .is_some_and(|head| !head.audible() && head.since + WATCHDOG_MS <= now)
        {
            self.fail(0, now);
        }
        // A line rendered ahead that never finished is said afresh; one no longer queued is
        // forgotten once its synthesis is over.
        let pending = &self.pending;
        self.prerender.retain(|ahead| match ahead.done {
            true => pending.contains(&ahead.utterance),
            false => ahead.since + WATCHDOG_MS > now,
        });
        if let Some((item, _, until)) = self.gap
            && until <= now
        {
            self.gap = None;
            self.parked = Some(item);
            self.think_until = Some(now + self.thinking_ms);
        }
        if self.yield_until.is_some_and(|until| until <= now) {
            // The duplex machine never decided: a paused pump must not stay paused, so the
            // actor performs this cut (it opens the gate) even with nothing left to play.
            self.yield_until = None;
            self.cut.get_or_insert((CutCause::SideTalkTimeout, false));
        }
        if self.turn_until.is_some_and(|(_, until)| until <= now) {
            self.turn_until = None;
        }
        if self.think_until.is_some_and(|until| until <= now) {
            self.think_until = None;
            self.parked = None;
        }
        // An urgent question takes the floor from a relayed answer at its next clause
        // boundary; the answer's sentences go back to the queue, to resume after it.
        let relay = |active: &Active| active.utterance.kind == Kind::Relay;
        if self.urgent_waits() && self.cut.is_none() && self.active.iter().any(relay) {
            let at = match self.preempt {
                Some(at) => at,
                None => *self.preempt.insert(self.clause_end(now)),
            };
            if at <= now {
                self.preempt = None;
                self.requeue = true;
                self.want_cut(now, CutCause::PreEmpted);
            }
        } else {
            self.preempt = None;
        }
    }

    /// When the playing line reaches its next clause boundary (a comma, a semicolon, a colon
    /// or a sentence end) within [`PREEMPT_MAX_MS`]; else when its current word ends. Now when
    /// nothing is audible.
    fn clause_end(&self, now: Ms) -> Ms {
        let Some(head) = self.active.front().filter(|head| head.audible()) else {
            return now;
        };
        let played = (now.saturating_sub(head.since) as f64 * self.chars_per_ms) as usize;
        let after = |stop: &dyn Fn(char) -> bool| {
            let rest = head.text.chars().enumerate().skip(played);
            rest.filter(|(_, c)| stop(*c)).map(|(at, _)| at + 1).next()
        };
        let wait = |at: usize| ((at.saturating_sub(played)) as f64 / self.chars_per_ms) as Ms;
        match after(&|c| matches!(c, ',' | ';' | ':' | '.' | '!' | '?')).map(wait) {
            Some(ms) if ms <= PREEMPT_MAX_MS => now + ms,
            _ => {
                now + after(&char::is_whitespace)
                    .map_or(0, wait)
                    .min(PREEMPT_MAX_MS)
            }
        }
    }

    /// Utterances `0..count` have been heard in full; the last of them ended at `at`.
    fn heard_through(&mut self, count: usize, at: Ms) {
        for active in self.active.drain(..count).collect::<Vec<_>>() {
            let utterance = active.utterance;
            if utterance.discard {
                continue;
            }
            // A confirmation's `heard_margin_ms` is added once, by `hitl::binds`.
            self.out.push(LaneEvent::Heard {
                item: utterance.item,
                heard_at: at + self.lead_ms + self.lag_ms,
            });
            // What a control says ("I don't have that to say again") is not what a repeat
            // says again.
            let control =
                matches!(&utterance.say, Say::Template { id, .. } if id.starts_with("control."));
            if !control {
                self.last_heard = Some(active.text.clone());
            }
            self.remember(at, active.text, false);
            self.resume(utterance.item);
            let more = self
                .active
                .iter()
                .any(|a| a.utterance.item == utterance.item)
                || self
                    .pending
                    .iter()
                    .any(|queued| queued.item == utterance.item);
            if utterance.sentence.is_some() && !utterance.last && !more {
                // Stream gap: the relay's next sentence is late.
                self.gap = Some((utterance.item, utterance.lane, at + STREAM_GAP_MS));
            }
        }
        if count > 0
            && let Some(head) = self.active.front_mut()
        {
            head.since = at;
        }
    }

    /// Speech ended: with nothing left to play or queued, and no relay holding a gap, the
    /// lanes are idle.
    fn lanes_idle(&mut self, at: Ms) {
        if self.nothing_queued() && self.gap.is_none() {
            self.out.push(LaneEvent::Idle { at });
        }
        self.rest_unless_owed(at);
    }

    /// With nothing at the provider, the floor keeps thinking only while more speech is owed.
    fn rest_unless_owed(&mut self, at: Ms) {
        if self.active.iter().all(|active| active.utterance.discard) {
            let owed = self.task_live || self.gap.is_some() || !self.nothing_queued();
            self.think_until = owed.then_some(at + self.thinking_ms);
            if !owed {
                self.parked = None;
            }
        }
    }

    fn fail(&mut self, index: usize, now: Ms) {
        let Some(failed) = self.active.remove(index) else {
            return;
        };
        if !failed.utterance.discard {
            self.out.push(LaneEvent::Dropped {
                item: failed.utterance.item,
                heard_chars: 0,
                cause: CutCause::Failure,
            });
            if failed.started {
                self.remember(now, failed.text, false);
            }
        }
        self.resume(failed.utterance.item);
        if let Some(head) = self.active.front_mut() {
            head.since = head.since.max(now);
        }
        self.lanes_idle(now);
    }

    /// The caller goes on with their turn (speaks again within `GOING_ON_MS` of its end, as a
    /// hesitant request does) before a word of the line with the provider was heard (a
    /// synthesis can take a second or two): the turn was not over, so the line is taken back
    /// rather than started over them. Words owed to the caller wait in the queue for their turn
    /// to end; an acknowledgement or a holding line is dropped (the clip drop rule, "if the
    /// caller resumed speaking"). Later than that the caller's speech is a new turn, and the
    /// line plays (a barge-in if it is over them). Not for an urgent question, not in a stream
    /// gap.
    fn take_back_unheard(&mut self, now: Ms) {
        let unheard = |active: &Active| !active.utterance.discard && !active.started;
        let urgent = |active: &Active| active.utterance.rank <= RANK_URGENT;
        let going_on = self.ended_at.is_some_and(|at| now <= at + GOING_ON_MS);
        if going_on
            && self.gap.is_none()
            && self.cut.is_none()
            && !self.active.iter().any(|active| active.started)
            && self.active.iter().any(unheard)
            && !self.active.iter().any(urgent)
        {
            self.taken_back = true;
            self.want_cut(now, CutCause::PreEmpted);
        }
    }

    fn want_cut(&mut self, now: Ms, cause: CutCause) {
        self.yield_until = None;
        if self.active.iter().all(|active| active.utterance.discard) {
            // Nothing audible is with the provider or the pump: no cut to perform.
            self.finish_cut(now, cause, &[], false);
        } else if self.cut.is_none() {
            self.cut = Some((cause, false));
        }
    }

    /// `performed`: the provider and the pump were cleared, so a warm-up in synthesis is
    /// gone too.
    fn finish_cut(&mut self, now: Ms, cause: CutCause, heard: &[(UttId, usize)], performed: bool) {
        // A cut for an urgent question gives the relayed answer back to the queue, and one for
        // a caller who went on with their turn every line owed to them (no other part says
        // them again; questions and updates the policy offers again itself).
        let preempted = cause == CutCause::PreEmpted;
        if !preempted {
            // The provider's synthesis is cleared too (only a pre-emption keeps it): what was
            // rendered ahead is said afresh, if at all.
            self.prerender.clear();
        }
        let requeue = std::mem::take(&mut self.requeue) && preempted;
        let taken_back = std::mem::take(&mut self.taken_back) && preempted;
        let owed = |kind: Kind| match kind {
            Kind::Relay => requeue || taken_back,
            Kind::Answer | Kind::Notice | Kind::Client => taken_back,
            _ => false,
        };
        let mut back = Vec::new();
        self.cut = None;
        self.yield_until = None;
        self.gap = None;
        self.parked = None;
        let (cut, kept): (VecDeque<_>, VecDeque<_>) = std::mem::take(&mut self.active)
            .into_iter()
            .partition(|active| performed || !active.utterance.discard);
        self.active = kept;
        for active in cut {
            let item = active.utterance.item;
            if active.utterance.discard {
                continue;
            }
            let heard_chars = heard
                .iter()
                .find(|(utt, _)| *utt == active.utt)
                .map_or(0, |(_, chars)| *chars);
            // A relayed sentence the caller heard most of is not said again whole after the
            // question that cut it; one cut early is. Live (take 15): "One's checking the
            // GitHub release, one's pulling the download numbers in the terminal, and one's
            // drafting the announcement." played from 30.2 s until a confirmation cut it at
            // 34.05 s (3.9 s of its 6.9 s), and was said again in full at 45.5 s after "Okay.
            // Anyway,".
            let total = active.text.chars().count();
            let mostly = active.started
                && requeue
                && active.utterance.kind == Kind::Relay
                && heard_chars * 100 >= total * MOSTLY_HEARD_PERCENT;
            if owed(active.utterance.kind) && !mostly {
                back.push(active.utterance.clone());
            }
            if active.started {
                self.out.push(LaneEvent::Dropped {
                    item,
                    heard_chars,
                    cause,
                });
                self.remember(now, active.text, cause == CutCause::Interrupt);
            } else {
                self.out.push(LaneEvent::Returned { item });
            }
        }
        if matches!(
            cause,
            CutCause::Interrupt | CutCause::SideTalkTimeout | CutCause::Client | CutCause::Shutdown
        ) {
            // The caller took the floor: what was queued is stale. Warm-ups stay.
            let (stale, warm): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending)
                .into_iter()
                .partition(|queued| !queued.discard);
            self.pending = warm;
            self.give_back(stale);
        }
        // Ahead of what was queued after them, in the order they had.
        for utterance in back.into_iter().rev() {
            let at = self
                .pending
                .iter()
                .position(|queued| queued.rank >= utterance.rank);
            self.pending
                .insert(at.unwrap_or(self.pending.len()), utterance);
        }
        self.lanes_idle(now);
    }

    fn give_back(&mut self, utterances: Vec<Utterance>) {
        self.out.extend(
            utterances
                .into_iter()
                .filter(|utterance| !utterance.discard)
                .map(|utterance| LaneEvent::Returned {
                    item: utterance.item,
                }),
        );
    }

    /// The relay of `item` has no more sentences.
    fn close_relay(&mut self, item: ItemId) {
        let queued = self
            .pending
            .iter_mut()
            .rev()
            .find(|queued| queued.item == item);
        let active = self
            .active
            .iter_mut()
            .rev()
            .map(|active| &mut active.utterance)
            .find(|utterance| utterance.item == item);
        match queued.or(active) {
            Some(utterance) => utterance.last = true,
            None => self.resume(item),
        }
    }

    /// `item` speaks again or is over: it no longer holds a gap.
    fn resume(&mut self, item: ItemId) {
        if self.gap.is_some_and(|gap| gap.0 == item) {
            self.gap = None;
        }
        if self.parked == Some(item) {
            self.parked = None;
        }
    }

    /// The words of `say`, the `n`th in its bank's rotation, unless that line shares a phrase
    /// with a line the agent said in the last [`SAID_LATELY_MS`]: then the next in the
    /// rotation that does not (the rotation's own pick when every one does). Live (take 15):
    /// "Thanks for waiting, I'm back." at 20.2 s, then the holding line "Still working on it,
    /// thanks for waiting." at 22.3 s.
    fn fresh(&self, now: Ms, say: &Say, n: u64) -> Option<String> {
        let lately: Vec<&str> = self
            .lines
            .iter()
            .filter(|(at, _)| *at + SAID_LATELY_MS > now)
            .map(|(_, line)| line.as_str())
            .collect();
        let count = self.persona.bank_choices(say) as u64;
        if lately.is_empty() || count < 2 {
            return self.persona.resolve(say, n);
        }
        let repeats = |line: &String| lately.iter().any(|said| guard::shares_phrase(line, said));
        (n..n + count)
            .filter_map(|n| self.persona.resolve(say, n))
            .find(|line| !repeats(line))
            .or_else(|| self.persona.resolve(say, n))
    }

    fn remember(&mut self, at: Ms, line: String, interrupted: bool) {
        if !line.is_empty() {
            self.last_interrupted = interrupted;
            self.lines.push_back((at, line));
            if self.lines.len() > LINES_KEPT {
                self.lines.pop_front();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::types::{
        Attribution, Class, Effects, EndOfTurn, EotSource, HitlId, HitlKind, HitlRequest, TaskId,
        TaskRef, TaskState, TaskView,
    };
    use super::*;

    fn machine() -> FloorMachine {
        FloorMachine::new(&InteractionConfig::default(), Persona::load("default"), 150)
    }

    fn line(item: u64, rank: u8, text: &str) -> Utterance {
        Utterance {
            item: ItemId(item),
            lane: Lane::Front,
            kind: Kind::Answer,
            rank,
            say: Say::Verbatim(text.to_owned()),
            discard: false,
            task: None,
            reply_to: None,
            question: None,
            expires_at: None,
            sentence: None,
            last: false,
        }
    }

    fn sentence(item: u64, seq: u32, text: &str, last: bool) -> Utterance {
        Utterance {
            lane: Lane::Main,
            kind: Kind::Relay,
            sentence: Some((1, seq)),
            last,
            ..line(item, 4, text)
        }
    }

    fn warm(item: u64) -> Utterance {
        Utterance {
            kind: Kind::Warm,
            discard: true,
            ..line(item, 9, "Okay, one moment.")
        }
    }

    fn end_of_turn(turn: u64, at: Ms) -> DuplexAction {
        DuplexAction::EndOfTurn(EndOfTurn {
            turn: TurnId(turn),
            at,
            acoustic_end: None,
            onset_at: None,
            text: "book a table".to_owned(),
            source: EotSource::SttSpeechFinal,
            eager_same: None,
            class: Class::Interrupt,
            attribution: Attribution::High,
            suspected_echo: false,
            unclear: false,
            foreign: false,
        })
    }

    /// The `Submit` of `next_cmds`, if any.
    fn submit(floor: &mut FloorMachine, now: Ms) -> Option<(UttId, ItemId)> {
        floor.next_cmds(now).into_iter().find_map(|cmd| match cmd {
            FloorCmd::Submit { utt, utterance, .. } => Some((utt, utterance.item)),
            FloorCmd::Cut(_) | FloorCmd::DropWarm(_) | FloorCmd::Prerender { .. } => None,
        })
    }

    /// Offers, submits and plays one utterance to its end at `end`.
    fn play(floor: &mut FloorMachine, utterance: Utterance, start: Ms, end: Ms) -> Vec<LaneEvent> {
        floor.offer(start, utterance);
        let (utt, _) = submit(floor, start).expect("submitted");
        floor.on_pump(start, PumpEvent::Started { utt, at: start });
        floor.on_pump(start, PumpEvent::SynthDone { utt });
        floor.on_pump(
            end,
            PumpEvent::Idle {
                utt,
                delivered_ms: end - start,
                at: end,
            },
        )
    }

    /// The `(item, text)` of the next `Submit`, if any.
    fn submitted(floor: &mut FloorMachine, now: Ms) -> Option<(UttId, ItemId, String)> {
        floor.next_cmds(now).into_iter().find_map(|cmd| match cmd {
            FloorCmd::Submit {
                utt,
                utterance,
                text,
            } => Some((utt, utterance.item, text)),
            FloorCmd::Cut(_) | FloorCmd::DropWarm(_) | FloorCmd::Prerender { .. } => None,
        })
    }

    #[test]
    fn a_line_not_yet_heard_is_taken_back_when_the_caller_goes_on() {
        // E at 1,000 ms; the answer is with the provider and no audio of it has come.
        let mut floor = machine();
        floor.on_duplex(1_000, &end_of_turn(1, 1_000));
        floor.on_settled(1_000, TurnId(1));
        floor.offer(1_000, sentence(1, 1, "The store opens at nine.", true));
        let (utt, ..) = submitted(&mut floor, 1_000).unwrap();
        floor.on_pump(1_100, PumpEvent::SynthDone { utt });
        // The caller goes on 600 ms after E: the line is taken back, not started over them.
        floor.on_duplex(1_600, &DuplexAction::UserSpeaking(true));
        assert_eq!(
            floor.next_cmds(1_600),
            vec![FloorCmd::Cut(CutCause::PreEmpted)]
        );
        let events = floor.on_cut(1_600, CutCause::PreEmpted, &[]);
        assert_eq!(events, vec![LaneEvent::Returned { item: ItemId(1) }]);
        assert_eq!(floor.next_cmds(1_700), vec![], "not while they speak");
        // Their turn ends: the line is said whole.
        floor.on_duplex(3_000, &end_of_turn(2, 3_000));
        floor.on_settled(3_000, TurnId(2));
        let (_, item, text) = submitted(&mut floor, 3_000).unwrap();
        assert_eq!(
            (item, text.as_str()),
            (ItemId(1), "The store opens at nine.")
        );

        // Later than that the caller's speech is a new turn: nothing is taken back (an
        // interruption, a listening word over the answer that is about to play).
        let mut floor = machine();
        floor.on_duplex(1_000, &end_of_turn(1, 1_000));
        floor.on_settled(1_000, TurnId(1));
        floor.offer(1_000, sentence(1, 1, "The store opens at nine.", true));
        let (utt, ..) = submitted(&mut floor, 1_000).unwrap();
        floor.on_pump(1_100, PumpEvent::SynthDone { utt });
        floor.on_duplex(2_600, &DuplexAction::UserSpeaking(true));
        assert_eq!(floor.next_cmds(2_600), vec![]);
        // Nor once a word of it was heard: that is a barge-in.
        let mut floor = machine();
        floor.on_duplex(1_000, &end_of_turn(1, 1_000));
        floor.on_settled(1_000, TurnId(1));
        floor.offer(1_000, sentence(1, 1, "The store opens at nine.", true));
        let (utt, ..) = submitted(&mut floor, 1_000).unwrap();
        floor.on_pump(1_100, PumpEvent::Started { utt, at: 1_100 });
        floor.on_duplex(1_600, &DuplexAction::UserSpeaking(true));
        assert_eq!(floor.next_cmds(1_600), vec![]);
    }

    #[test]
    fn an_urgent_question_cuts_at_a_clause_and_the_answer_resumes() {
        let mut floor = machine();
        floor.set_speech_rate(0.015);
        // Two sentences of an answer: the first plays, the second waits with the provider.
        let first = "The capital of Peru is Lima, on the coast.";
        floor.offer(0, sentence(1, 1, first, false));
        floor.offer(0, sentence(1, 2, "It has ten million people.", true));
        let (playing, ..) = submitted(&mut floor, 0).unwrap();
        floor.on_pump(0, PumpEvent::SynthDone { utt: playing });
        floor.on_pump(
            100,
            PumpEvent::Started {
                utt: playing,
                at: 100,
            },
        );
        let (queued, ..) = submitted(&mut floor, 100).unwrap();
        floor.on_pump(150, PumpEvent::SynthDone { utt: queued });
        // An urgent question arrives 900 ms into the first sentence (13 characters heard):
        // the comma after "Lima" is 15 characters on, 1 s away; within 400 ms there is only
        // the end of the word "of", so that is where it cuts.
        let question = Utterance {
            kind: Kind::Question,
            question: Some(HitlId("q1".to_owned())),
            ..line(9, RANK_URGENT, "Window or aisle?")
        };
        floor.offer(1_000, question);
        // Not cut yet (mid-word), but its words go to synthesis now, into the speech cache.
        let ahead = match floor.next_cmds(1_000).as_slice() {
            [FloorCmd::Prerender { utt, text }] if text == "Window or aisle?" => *utt,
            other => panic!("the question rendered ahead, got {other:?}"),
        };
        assert!(floor.prerendering());
        floor.on_pump(1_000, PumpEvent::SynthDone { utt: ahead });
        assert!(!floor.prerendering());
        assert_eq!(
            floor.next_cmds(1_000),
            vec![],
            "rendered once, and not cut yet"
        );
        let at = floor.next_wakeup().expect("the cut is timed");
        assert!((1_000..=1_400).contains(&at), "{at}");
        assert_eq!(
            floor.next_cmds(at),
            vec![FloorCmd::Cut(CutCause::PreEmpted)]
        );
        let events = floor.on_cut(at, CutCause::PreEmpted, &[(playing, 14)]);
        assert_eq!(
            events,
            vec![
                LaneEvent::Dropped {
                    item: ItemId(1),
                    heard_chars: 14,
                    cause: CutCause::PreEmpted
                },
                LaneEvent::Returned { item: ItemId(1) },
            ]
        );
        // The question goes first, as the utterance rendered ahead (its words in the speech
        // cache); the answer is queued again from the sentence that was cut, which the caller
        // did not hear in full.
        let (asked, item, text) = submitted(&mut floor, at).unwrap();
        assert_eq!(
            (asked, item, text.as_str()),
            (ahead, ItemId(9), "Window or aisle?")
        );
        floor.on_board(
            at,
            &Board {
                open_question: Some(HitlRequest::default()),
                ..Board::default()
            },
        );
        floor.on_pump(at, PumpEvent::SynthDone { utt: asked });
        floor.on_pump(
            at + 10,
            PumpEvent::Started {
                utt: asked,
                at: at + 10,
            },
        );
        floor.on_pump(
            at + 1_000,
            PumpEvent::Idle {
                utt: asked,
                delivered_ms: 990,
                at: at + 1_000,
            },
        );
        assert_eq!(floor.next_cmds(at + 1_000), vec![], "held while it is open");
        floor.on_board(at + 3_000, &Board::default());
        let (_, item, text) = submitted(&mut floor, at + 3_000).unwrap();
        assert_eq!((item, text.as_str()), (ItemId(1), first));
        assert_eq!(floor.relay_items(), vec![ItemId(1)]);
    }

    /// Live (launch demo take 15): a relayed sentence played from 30.2 s until a confirmation
    /// cut it at 34.05 s, 3.9 s of its 6.9 s heard, and after the question it was said again
    /// in full ("Okay. Anyway, One's checking the GitHub release, ..."). A sentence the caller
    /// heard most of is not said again; what had not started yet still resumes.
    #[test]
    fn an_urgent_question_does_not_say_again_a_sentence_mostly_heard() {
        let first = "One's checking the GitHub release, one's pulling the download numbers in \
                     the terminal, and one's drafting the announcement.";
        let cut_after = |heard: usize, second: bool| {
            let mut floor = machine();
            floor.set_speech_rate(0.015);
            floor.offer(0, sentence(1, 1, first, !second));
            if second {
                floor.offer(
                    0,
                    sentence(1, 2, "I'll tell you what each one finds.", true),
                );
            }
            let (playing, ..) = submitted(&mut floor, 0).unwrap();
            floor.on_pump(0, PumpEvent::SynthDone { utt: playing });
            floor.on_pump(
                100,
                PumpEvent::Started {
                    utt: playing,
                    at: 100,
                },
            );
            if second {
                let (queued, ..) = submitted(&mut floor, 100).unwrap();
                floor.on_pump(150, PumpEvent::SynthDone { utt: queued });
            }
            let question = Utterance {
                kind: Kind::Confirm,
                question: Some(HitlId("q1".to_owned())),
                ..line(9, RANK_URGENT, "To confirm: save the document.")
            };
            floor.offer(3_900, question);
            // Rendered ahead, then cut at the next word end.
            let mut ahead = None;
            let at = (3_900..6_000)
                .find(|now| {
                    let cmds = floor.next_cmds(*now);
                    for cmd in &cmds {
                        if let FloorCmd::Prerender { utt, .. } = cmd {
                            ahead = Some(*utt);
                        }
                    }
                    cmds.contains(&FloorCmd::Cut(CutCause::PreEmpted))
                })
                .expect("cut for the question");
            let events = floor.on_cut(at, CutCause::PreEmpted, &[(playing, heard)]);
            if let Some(utt) = ahead {
                floor.on_pump(at, PumpEvent::SynthDone { utt });
            }
            (floor, events)
        };
        // 61 of its 123 characters heard: not said again, and nothing of the answer is left
        // to resume (no "Anyway," then: the policy says it only for a queued relay).
        let (floor, events) = cut_after(61, false);
        assert_eq!(
            events,
            vec![LaneEvent::Dropped {
                item: ItemId(1),
                heard_chars: 61,
                cause: CutCause::PreEmpted
            }]
        );
        assert_eq!(floor.relay_items(), vec![]);
        // The sentence after it, not yet heard, still resumes after the question.
        let (mut floor, _) = cut_after(61, true);
        assert_eq!(floor.relay_items(), vec![ItemId(1)]);
        let (_, item, text) = submitted(&mut floor, 6_000).unwrap();
        assert_eq!(
            (item, text.as_str()),
            (ItemId(9), "To confirm: save the document.")
        );
        let pending: Vec<&str> = floor
            .pending
            .iter()
            .map(|queued| match &queued.say {
                Say::Verbatim(text) => text.as_str(),
                _ => "",
            })
            .collect();
        assert_eq!(pending, ["I'll tell you what each one finds."]);
        // Cut early, it is said again, as before.
        let (floor, _) = cut_after(20, false);
        assert_eq!(floor.relay_items(), vec![ItemId(1)]);
    }

    /// Live (launch demo take 15): "Thanks for waiting, I'm back." (a failure line, said
    /// apart from the floor) at 20.2 s, then the holding line "Still working on it, thanks
    /// for waiting." at 22.3 s. A bank line that shares a phrase with a line said lately gives
    /// way to the next of its bank.
    #[test]
    fn a_bank_line_does_not_repeat_a_phrase_just_said() {
        let hold = |floor: &mut FloorMachine, now: Ms| {
            let utterance = Utterance {
                kind: Kind::Hold,
                say: Say::Bank {
                    group: "holding",
                    intent: None,
                },
                ..line(1, 8, "")
            };
            floor.offer(now, utterance);
            submitted(floor, now).unwrap().2
        };
        // The rotation's own pick for the first utterance.
        let alone = hold(&mut machine(), 1_000);
        assert_eq!(alone, "Still working on it, thanks for waiting.");
        let mut floor = machine();
        floor.said_apart(20_200, "Thanks for waiting, I'm back.".to_owned());
        let said = hold(&mut floor, 22_300);
        assert_eq!(said, "I'm still on it.");
        assert!(
            floor
                .snapshot()
                .recent_agent_lines
                .contains(&(20_200, "Thanks for waiting, I'm back.".to_owned()))
        );
        // Long enough after it, the rotation's pick again.
        let mut floor = machine();
        floor.said_apart(1_000, "Thanks for waiting, I'm back.".to_owned());
        assert_eq!(hold(&mut floor, 1_000 + SAID_LATELY_MS), alone);
    }

    /// An urgent line in synthesis ahead of its cut holds the provider: nothing is submitted
    /// until its synthesis is over, a cut for it keeps it, and any other cut forgets it.
    #[test]
    fn an_urgent_line_rendered_ahead_holds_the_provider_until_it_is_synthesised() {
        let urgent = |floor: &mut FloorMachine, now: Ms| {
            let first = "The capital of Peru is Lima, on the coast.";
            floor.offer(now, sentence(1, 1, first, true));
            let (playing, ..) = submitted(floor, now).unwrap();
            floor.on_pump(now, PumpEvent::SynthDone { utt: playing });
            floor.on_pump(
                now + 100,
                PumpEvent::Started {
                    utt: playing,
                    at: now + 100,
                },
            );
            let question = Utterance {
                kind: Kind::Question,
                question: Some(HitlId("q1".to_owned())),
                ..line(9, RANK_URGENT, "Window or aisle?")
            };
            floor.offer(now + 1_000, question);
            match floor.next_cmds(now + 1_000).as_slice() {
                [FloorCmd::Prerender { utt, .. }] => (playing, *utt),
                other => panic!("rendered ahead, got {other:?}"),
            }
        };
        let mut floor = machine();
        floor.set_speech_rate(0.015);
        let (playing, ahead) = urgent(&mut floor, 0);
        let at = floor.next_wakeup().unwrap();
        assert_eq!(
            floor.next_cmds(at),
            vec![FloorCmd::Cut(CutCause::PreEmpted)]
        );
        assert!(floor.prerendering(), "the pre-emption keeps its synthesis");
        floor.on_cut(at, CutCause::PreEmpted, &[(playing, 14)]);
        assert_eq!(
            submitted(&mut floor, at),
            None,
            "the provider is still on it"
        );
        floor.on_pump(at + 300, PumpEvent::SynthDone { utt: ahead });
        let (asked, item, _) = submitted(&mut floor, at + 300).unwrap();
        assert_eq!((asked, item), (ahead, ItemId(9)));

        // The caller's barge-in clears the provider: the line is said afresh, if at all.
        let mut floor = machine();
        floor.set_speech_rate(0.015);
        let (playing, ahead) = urgent(&mut floor, 0);
        floor.on_duplex(1_050, &DuplexAction::Cut(CutCause::Interrupt));
        assert_eq!(
            floor.next_cmds(1_050),
            vec![FloorCmd::Cut(CutCause::Interrupt)]
        );
        floor.on_cut(1_050, CutCause::Interrupt, &[(playing, 14)]);
        assert!(!floor.prerendering());
        floor.on_pump(1_100, PumpEvent::SynthDone { utt: ahead });
        assert!(!floor.prerendering());
    }

    /// An urgent line without words (a pack without the line) has nothing to render ahead:
    /// the floor does not wake for it at once, again and again, while the answer plays.
    #[test]
    fn an_urgent_line_without_words_is_not_rendered_ahead() {
        let mut floor = machine();
        floor.set_speech_rate(0.015);
        floor.offer(
            0,
            sentence(1, 1, "The capital of Peru is Lima, on the coast.", true),
        );
        let (playing, ..) = submitted(&mut floor, 0).unwrap();
        floor.on_pump(0, PumpEvent::SynthDone { utt: playing });
        floor.on_pump(
            100,
            PumpEvent::Started {
                utt: playing,
                at: 100,
            },
        );
        floor.offer(1_000, line(9, RANK_URGENT, " "));
        assert_eq!(floor.next_cmds(1_000), vec![]);
        assert!(!floor.prerendering());
        assert!(
            floor.next_wakeup().is_some_and(|at| at > 1_000),
            "{:?}",
            floor.next_wakeup()
        );
    }

    #[test]
    fn an_urgent_question_never_starts_over_the_caller_but_in_a_pause() {
        let mut floor = machine();
        floor.on_duplex(0, &DuplexAction::UserSpeaking(true));
        floor.offer(0, line(1, 2, "A reply."));
        let urgent = Utterance {
            question: Some(HitlId("q1".to_owned())),
            ..line(2, RANK_URGENT, "Window or aisle?")
        };
        floor.offer(0, urgent);
        assert_eq!(floor.next_cmds(100), vec![]);
        // The caller pauses: 200 ms are not enough, 250 ms are.
        floor.on_voice(false, 1_000);
        assert_eq!(floor.next_cmds(1_200), vec![]);
        assert_eq!(floor.next_wakeup(), Some(1_250));
        let (utt, item, _) = submitted(&mut floor, 1_250).unwrap();
        assert_eq!(item, ItemId(2), "only the urgent question");
        floor.on_pump(1_260, PumpEvent::SynthDone { utt });
        // The reply never starts over the caller, pause or not.
        assert_eq!(floor.next_cmds(1_300), vec![]);
        floor.on_voice(true, 1_400);
        assert_eq!(floor.next_cmds(2_000), vec![]);
    }

    #[test]
    fn second_submit_only_after_the_first_synth_done() {
        let mut floor = machine();
        floor.offer(0, line(1, 2, "One."));
        floor.offer(0, line(2, 2, "Two."));
        floor.offer(0, line(3, 2, "Three."));
        let (first, item) = submit(&mut floor, 0).unwrap();
        assert_eq!(item, ItemId(1));
        assert_eq!(floor.next_cmds(5), vec![]);
        // Audio alone does not free the slot.
        floor.on_pump(10, PumpEvent::Started { utt: first, at: 10 });
        assert_eq!(floor.next_cmds(10), vec![]);
        floor.on_pump(20, PumpEvent::SynthDone { utt: first });
        let (second, item) = submit(&mut floor, 20).unwrap();
        assert_eq!(item, ItemId(2));
        assert_eq!(floor.next_cmds(21), vec![]);
        // A failure frees it too.
        let events = floor.on_pump(30, PumpEvent::Failed { utt: second });
        assert_eq!(
            events,
            vec![LaneEvent::Dropped {
                item: ItemId(2),
                heard_chars: 0,
                cause: CutCause::Failure
            }]
        );
        assert_eq!(submit(&mut floor, 30).unwrap().1, ItemId(3));
    }

    #[test]
    fn the_agent_is_quiet_from_its_last_line_unless_the_caller_cut_it() {
        let mut floor = machine();
        assert_eq!(floor.quiet_since(), None);
        play(&mut floor, line(1, 2, "One moment."), 0, 800);
        assert_eq!(floor.quiet_since(), Some(800));
        // Live MA5: the caller went on over the acknowledgement; they were the one talking.
        floor.offer(1_000, line(2, 2, "One moment."));
        let (playing, _) = submit(&mut floor, 1_000).unwrap();
        let started = PumpEvent::Started {
            utt: playing,
            at: 1_000,
        };
        floor.on_pump(1_000, started);
        floor.on_cut(1_300, CutCause::Interrupt, &[(playing, 3)]);
        assert_eq!(floor.last_line_at(), Some(1_300));
        assert_eq!(floor.quiet_since(), None);
        let update = line(3, 2, "Checking the tables now.");
        play(&mut floor, update, 2_000, 3_000);
        assert_eq!(floor.quiet_since(), Some(3_000));
    }

    #[test]
    fn cut_drops_the_playing_and_returns_the_queued() {
        let mut floor = machine();
        for (item, text) in [(1, "One."), (2, "Two."), (3, "Three.")] {
            floor.offer(0, line(item, 2, text));
        }
        let (playing, _) = submit(&mut floor, 0).unwrap();
        floor.on_pump(
            10,
            PumpEvent::Started {
                utt: playing,
                at: 10,
            },
        );
        floor.on_pump(20, PumpEvent::SynthDone { utt: playing });
        let (in_synthesis, _) = submit(&mut floor, 20).unwrap();

        assert_eq!(
            floor.on_duplex(100, &DuplexAction::Cut(CutCause::Interrupt)),
            vec![]
        );
        assert_eq!(
            floor.next_cmds(100),
            vec![FloorCmd::Cut(CutCause::Interrupt)]
        );
        let events = floor.on_cut(100, CutCause::Interrupt, &[(playing, 3)]);
        assert_eq!(
            events,
            vec![
                LaneEvent::Dropped {
                    item: ItemId(1),
                    heard_chars: 3,
                    cause: CutCause::Interrupt
                },
                LaneEvent::Returned { item: ItemId(2) },
                LaneEvent::Returned { item: ItemId(3) },
                LaneEvent::Idle { at: 100 },
            ]
        );
        // Late pump events of the cut utterances are ignored.
        for utt in [playing, in_synthesis] {
            assert_eq!(floor.on_pump(110, PumpEvent::SynthDone { utt }), vec![]);
            assert_eq!(
                floor.on_pump(
                    110,
                    PumpEvent::Idle {
                        utt,
                        delivered_ms: 90,
                        at: 110
                    }
                ),
                vec![]
            );
        }
        assert_eq!(floor.snapshot().floor, Floor::Listening);
        // Cut then speak: the new line is submitted at once.
        floor.offer(120, line(4, 2, "Scratch that."));
        assert_eq!(submit(&mut floor, 120).unwrap().1, ItemId(4));
    }

    #[test]
    fn an_unreported_cut_completes_on_the_next_run() {
        let mut floor = machine();
        floor.offer(0, line(1, 2, "One."));
        let (utt, _) = submit(&mut floor, 0).unwrap();
        floor.on_pump(10, PumpEvent::Started { utt, at: 10 });
        floor.on_duplex(50, &DuplexAction::Cut(CutCause::Interrupt));
        assert_eq!(
            floor.next_cmds(50),
            vec![FloorCmd::Cut(CutCause::Interrupt)]
        );
        assert_eq!(floor.next_wakeup(), Some(50));
        assert_eq!(
            floor.tick(51),
            vec![
                LaneEvent::Dropped {
                    item: ItemId(1),
                    heard_chars: 0,
                    cause: CutCause::Interrupt
                },
                LaneEvent::Idle { at: 51 },
            ]
        );
    }

    #[test]
    fn dropping_a_playing_item_cuts_and_keeps_other_items_queued() {
        let mut floor = machine();
        floor.offer(0, sentence(7, 0, "It leaves at nine.", false));
        floor.offer(0, sentence(7, 1, "It costs forty dollars.", true));
        floor.offer(0, line(8, 2, "Scratch that."));
        let (utt, _) = submit(&mut floor, 0).unwrap();
        // Rank 2 was offered last but goes first.
        assert_eq!(floor.active[0].utterance.item, ItemId(8));
        floor.on_pump(1, PumpEvent::Started { utt, at: 1 });
        floor.on_pump(2, PumpEvent::SynthDone { utt });
        let (relay, _) = submit(&mut floor, 2).unwrap();
        floor.on_pump(
            900,
            PumpEvent::Started {
                utt: relay,
                at: 900,
            },
        );

        floor.offer(1000, line(9, 2, "Okay, I'll change that."));
        assert_eq!(
            floor.drop_item(1000, ItemId(7)),
            vec![LaneEvent::Returned { item: ItemId(7) }]
        );
        assert_eq!(
            floor.next_cmds(1000),
            vec![FloorCmd::Cut(CutCause::PreEmpted)]
        );
        assert_eq!(
            floor.on_cut(1000, CutCause::PreEmpted, &[(relay, 9)]),
            vec![LaneEvent::Dropped {
                item: ItemId(7),
                heard_chars: 9,
                cause: CutCause::PreEmpted
            }]
        );
        assert_eq!(submit(&mut floor, 1000).unwrap().1, ItemId(9));
    }

    #[test]
    fn the_watchdog_fails_a_silent_synthesis() {
        let mut floor = machine();
        floor.offer(0, line(1, 2, "One."));
        floor.offer(0, line(2, 2, "Two."));
        submit(&mut floor, 0).unwrap();
        assert_eq!(floor.next_wakeup(), Some(10_000));
        assert_eq!(floor.tick(9_999), vec![]);
        assert_eq!(
            floor.tick(10_000),
            vec![LaneEvent::Dropped {
                item: ItemId(1),
                heard_chars: 0,
                cause: CutCause::Failure
            }]
        );
        assert_eq!(submit(&mut floor, 10_000).unwrap().1, ItemId(2));
    }

    #[test]
    fn a_warm_up_never_takes_the_floor() {
        let mut floor = machine();
        floor.offer(0, warm(1));
        floor.offer(0, warm(2));
        let cmds = floor.next_cmds(0);
        let [FloorCmd::Submit { utt, utterance, .. }] = &cmds[..] else {
            panic!("expected one warm-up submit, got {cmds:?}");
        };
        assert!(utterance.discard);
        let utt = *utt;
        assert_eq!(floor.snapshot().floor, Floor::Listening);
        assert_eq!(floor.wire_state(), ("none", None, "idle"));

        // A real line does not wait for that phrase (live L1: an acknowledgement waited up to
        // 2.3 s for a failure line's render): its synthesis is dropped and it waits again.
        floor.offer(100, line(3, 2, "Sure, let me check."));
        let cmds = floor.next_cmds(100);
        assert!(
            matches!(
                &cmds[..],
                [FloorCmd::DropWarm(dropped), FloorCmd::Submit { utterance, .. }]
                    if *dropped == vec![utt] && utterance.item == ItemId(3)
            ),
            "{cmds:?}"
        );

        // The warm-ups wait until the floor is listening again, the dropped one after the other.
        assert_eq!(floor.next_cmds(101), vec![]);
        let utt = floor.active[0].utt;
        floor.on_pump(110, PumpEvent::Started { utt, at: 110 });
        floor.on_pump(120, PumpEvent::SynthDone { utt });
        assert_eq!(floor.next_cmds(120), vec![]);
        floor.on_pump(
            900,
            PumpEvent::Idle {
                utt,
                delivered_ms: 590,
                at: 900,
            },
        );
        assert_eq!(floor.snapshot().floor, Floor::Listening);
        assert!(matches!(
            &floor.next_cmds(900)[..],
            [FloorCmd::Submit { utterance, .. }] if utterance.discard && utterance.item == ItemId(2)
        ));
    }

    #[test]
    fn a_listening_sound_wanted_in_a_long_turn_is_made_in_it() {
        // The bank's sounds and a failure line queue behind a warm-up line while the caller
        // tells a long story.
        let clip = |item: u64, kind: Kind| Utterance {
            kind,
            rank: 10,
            discard: true,
            ..line(item, 10, "mhm")
        };
        let mut floor = machine();
        floor.offer(0, warm(1));
        floor.offer(0, clip(2, Kind::Failure));
        floor.offer(0, clip(3, Kind::Clip));
        floor.on_duplex(0, &DuplexAction::UserSpeaking(true));
        assert_eq!(
            submit(&mut floor, 0),
            None,
            "nothing while no sound is wanted"
        );
        // A pause the emitter wanted a listening sound in: the sound is made now, and only it.
        let wanted = DuplexAction::Backchannel {
            text: "so we are planning a trip".to_owned(),
            voiced_ms: 2_000,
        };
        floor.on_duplex(4_000, &wanted);
        let (utt, item) = submit(&mut floor, 4_000).expect("the listening sound");
        assert_eq!(item, ItemId(3));
        floor.on_pump(4_100, PumpEvent::SynthDone { utt });
        assert_eq!(
            submit(&mut floor, 4_100),
            None,
            "the warm-up and the failure line wait"
        );
        floor.on_duplex(9_000, &end_of_turn(1, 9_000));
        assert_eq!(
            submit(&mut floor, 9_000),
            None,
            "nor while the turn is decided"
        );
    }

    #[test]
    fn a_warm_up_waits_while_the_caller_speaks_or_a_task_runs() {
        let mut floor = machine();
        floor.offer(0, warm(1));
        floor.on_duplex(0, &DuplexAction::UserSpeaking(true));
        assert_eq!(floor.next_cmds(0), vec![]);
        floor.on_duplex(10, &DuplexAction::UserSpeaking(false));
        let task = TaskView {
            id: TaskId("task_1".to_owned()),
            task_ref: TaskRef("t1".to_owned()),
            revision: 1,
            title: "table".to_owned(),
            state: TaskState::Working,
            cancelling: false,
            effects: Effects::NoneCommitted,
            open_question: None,
            side_effects: false,
            started_at: 0,
            lineage: Default::default(),
        };
        floor.on_board(
            10,
            &Board {
                tasks: vec![task],
                ..Board::default()
            },
        );
        assert_eq!(floor.snapshot().floor, Floor::Thinking);
        assert_eq!(floor.next_cmds(10), vec![]);
        floor.on_board(20, &Board::default());
        assert_eq!(floor.snapshot().floor, Floor::Listening);
        assert_eq!(floor.next_cmds(20).len(), 1);
    }

    #[test]
    fn a_late_sentence_parks_the_relay_and_the_next_one_resumes_it() {
        let mut floor = machine();
        let events = play(
            &mut floor,
            sentence(7, 0, "It leaves at nine.", false),
            0,
            1_000,
        );
        assert_eq!(
            events,
            vec![LaneEvent::Heard {
                item: ItemId(7),
                heard_at: 1_300
            }]
        );
        // The relay keeps the floor for 600 ms; a holding line may not slip in.
        floor.offer(
            1_000,
            Utterance {
                kind: Kind::Hold,
                ..line(8, 8, "Still on it.")
            },
        );
        assert_eq!(floor.snapshot().floor, Floor::MainSpeaking);
        assert_eq!(floor.next_cmds(1_100), vec![]);
        assert_eq!(floor.next_wakeup(), Some(1_600));
        floor.drop_item(1_100, ItemId(8));

        assert_eq!(floor.tick(1_600), vec![]);
        assert_eq!(floor.snapshot().floor, Floor::Thinking);
        assert_eq!(floor.wire_state(), ("none", None, "stream_gap"));

        floor.offer(2_000, sentence(7, 1, "It costs forty dollars.", false));
        let (utt, _) = submit(&mut floor, 2_000).unwrap();
        floor.on_pump(2_050, PumpEvent::Started { utt, at: 2_050 });
        assert_eq!(floor.snapshot().floor, Floor::MainSpeaking);
        assert_eq!(floor.wire_state(), ("assistant", Some(Lane::Main), "relay"));

        // The terminal task event closes the relay: no gap after the last sentence.
        floor.offer(2_100, sentence(7, 2, "", true));
        floor.on_pump(2_100, PumpEvent::SynthDone { utt });
        let events = floor.on_pump(
            3_000,
            PumpEvent::Idle {
                utt,
                delivered_ms: 950,
                at: 3_000,
            },
        );
        assert_eq!(
            events,
            vec![
                LaneEvent::Heard {
                    item: ItemId(7),
                    heard_at: 3_300
                },
                LaneEvent::Idle { at: 3_000 }
            ]
        );
        assert_eq!(floor.snapshot().floor, Floor::Listening);
        assert_eq!(floor.next_wakeup(), None);
    }

    #[test]
    fn audio_that_runs_dry_before_the_synthesis_ends_is_a_stream_gap() {
        let mut floor = machine();
        floor.offer(0, line(1, 2, "A long answer."));
        let (utt, _) = submit(&mut floor, 0).unwrap();
        floor.on_pump(10, PumpEvent::Started { utt, at: 10 });
        assert_eq!(
            floor.on_pump(
                500,
                PumpEvent::Idle {
                    utt,
                    delivered_ms: 490,
                    at: 500
                }
            ),
            vec![]
        );
        assert!(!floor.snapshot().agent_audible);
        assert_eq!(floor.snapshot().floor, Floor::FrontSpeaking);
        floor.tick(1_100);
        assert_eq!(floor.snapshot().floor, Floor::Thinking);
        // More audio resumes it; the end of the synthesis after a dry pump ends it.
        floor.on_pump(1_200, PumpEvent::Started { utt, at: 1_200 });
        assert_eq!(floor.snapshot().floor, Floor::FrontSpeaking);
        // Audio that comes back within the gap takes the gap's mark with it.
        let dry = PumpEvent::Idle {
            utt,
            delivered_ms: 700,
            at: 1_400,
        };
        floor.on_pump(1_400, dry);
        assert!(floor.gap.is_some());
        floor.on_pump(1_450, PumpEvent::Started { utt, at: 1_450 });
        assert!(floor.gap.is_none());
        floor.on_pump(
            1_700,
            PumpEvent::Idle {
                utt,
                delivered_ms: 990,
                at: 1_700,
            },
        );
        assert_eq!(
            floor.on_pump(1_800, PumpEvent::SynthDone { utt }),
            vec![
                LaneEvent::Heard {
                    item: ItemId(1),
                    heard_at: 2_000
                },
                LaneEvent::Idle { at: 1_700 }
            ]
        );
    }

    #[test]
    fn a_synthesis_that_ends_without_audio_is_a_failure() {
        let mut floor = machine();
        floor.offer(0, line(1, 2, "One."));
        let (utt, _) = submit(&mut floor, 0).unwrap();
        // The pump closes an utterance it never played with `SynthDone` then `Idle`.
        assert_eq!(floor.on_pump(400, PumpEvent::SynthDone { utt }), vec![]);
        let idle = PumpEvent::Idle {
            utt,
            delivered_ms: 0,
            at: 400,
        };
        assert_eq!(
            floor.on_pump(400, idle),
            vec![
                LaneEvent::Dropped {
                    item: ItemId(1),
                    heard_chars: 0,
                    cause: CutCause::Failure
                },
                LaneEvent::Idle { at: 400 }
            ]
        );
        assert_eq!(floor.snapshot().floor, Floor::Listening);
    }

    #[test]
    fn heard_at_adds_lead_and_lag_and_leaves_the_confirmation_margin_to_the_binding_check() {
        let mut floor = machine();
        let events = play(&mut floor, line(1, 2, "Okay."), 0, 1_000);
        assert_eq!(
            events[0],
            LaneEvent::Heard {
                item: ItemId(1),
                heard_at: 1_300
            }
        );
        let readback = Utterance {
            kind: Kind::Confirm,
            ..line(2, 3, "A table for two.")
        };
        let events = play(&mut floor, readback, 2_000, 3_000);
        assert_eq!(
            events[0],
            LaneEvent::Heard {
                item: ItemId(2),
                heard_at: 3_300
            }
        );
        // Both lines are on the heard timeline with the time their playout ended.
        assert_eq!(
            floor.snapshot().recent_agent_lines,
            vec![
                (1_000, "Okay.".to_owned()),
                (3_000, "A table for two.".to_owned())
            ]
        );
    }

    #[test]
    fn a_queued_utterance_is_heard_when_the_next_one_starts() {
        let mut floor = machine();
        floor.offer(0, line(1, 2, "One."));
        floor.offer(0, line(2, 2, "Two."));
        let (first, _) = submit(&mut floor, 0).unwrap();
        floor.on_pump(10, PumpEvent::Started { utt: first, at: 10 });
        floor.on_pump(20, PumpEvent::SynthDone { utt: first });
        let (second, _) = submit(&mut floor, 20).unwrap();
        floor.on_pump(30, PumpEvent::SynthDone { utt: second });
        let events = floor.on_pump(
            600,
            PumpEvent::Started {
                utt: second,
                at: 600,
            },
        );
        assert_eq!(
            events,
            vec![
                LaneEvent::Heard {
                    item: ItemId(1),
                    heard_at: 900
                },
                LaneEvent::Started {
                    item: ItemId(2),
                    utt: second,
                    at: 600
                }
            ]
        );
        assert_eq!(floor.snapshot().playing, Some((ItemId(2), Kind::Answer)));
    }

    #[test]
    fn interlocks_hold_speech_back() {
        // Nothing starts while the caller speaks.
        let mut floor = machine();
        floor.on_duplex(0, &DuplexAction::UserSpeaking(true));
        floor.offer(0, line(1, 2, "Sure."));
        assert_eq!(floor.next_cmds(0), vec![]);
        assert_eq!(floor.snapshot().floor, Floor::UserSpeaking);

        // Ranks 3 to 8 wait for the decision of the latest turn, until E + 500 ms.
        floor.on_duplex(1_000, &end_of_turn(1, 1_000));
        floor.offer(1_000, sentence(2, 0, "It leaves at nine.", true));
        assert_eq!(submit(&mut floor, 1_000).unwrap().1, ItemId(1));
        let utt = floor.active[0].utt;
        floor.on_pump(1_010, PumpEvent::SynthDone { utt });
        assert_eq!(floor.next_cmds(1_010), vec![]);
        floor.on_settled(1_100, TurnId(1));
        assert_eq!(submit(&mut floor, 1_100).unwrap().1, ItemId(2));

        let mut floor = machine();
        floor.on_duplex(1_000, &end_of_turn(1, 1_000));
        floor.offer(1_000, sentence(2, 0, "It leaves at nine.", true));
        assert_eq!(floor.next_cmds(1_499), vec![]);
        assert_eq!(floor.next_wakeup(), Some(1_500));
        assert_eq!(submit(&mut floor, 1_500).unwrap().1, ItemId(2));

        // While a question is open only the question's own utterances start.
        let mut floor = machine();
        let question = HitlRequest {
            id: HitlId("q_1".to_owned()),
            task: TaskId("task_1".to_owned()),
            revision: 1,
            kind: HitlKind::Confirm,
            op: None,
            verb_phrase: "book a table".to_owned(),
            params: Vec::new(),
            question: None,
            options: Vec::new(),
            irreversible: true,
            expires_at: 20_000,
            ..Default::default()
        };
        floor.on_board(
            0,
            &Board {
                open_question: Some(question),
                ..Board::default()
            },
        );
        floor.offer(0, sentence(2, 0, "It leaves at nine.", true));
        assert_eq!(floor.next_cmds(0), vec![]);
        let ask = Utterance {
            question: Some(HitlId("q_1".to_owned())),
            ..line(3, 3, "Do you want that?")
        };
        floor.offer(0, ask);
        assert_eq!(submit(&mut floor, 0).unwrap().1, ItemId(3));

        // An expired utterance is returned.
        let mut floor = machine();
        floor.on_duplex(0, &DuplexAction::UserSpeaking(true));
        floor.offer(
            0,
            Utterance {
                expires_at: Some(3_000),
                ..line(4, 7, "Checking flights.")
            },
        );
        assert_eq!(floor.tick(2_999), vec![]);
        assert_eq!(
            floor.tick(3_000),
            vec![LaneEvent::Returned { item: ItemId(4) }]
        );
    }

    #[test]
    fn an_onset_over_agent_audio_yields_until_decided_or_the_backstop() {
        let mut floor = machine();
        floor.offer(0, line(1, 2, "A long answer."));
        let (utt, _) = submit(&mut floor, 0).unwrap();
        // A duck with no agent audio is not a yield.
        floor.on_duplex(5, &DuplexAction::Gate(GateCmd::Duck));
        assert_eq!(floor.snapshot().floor, Floor::Thinking);
        floor.on_pump(10, PumpEvent::Started { utt, at: 10 });
        floor.on_duplex(100, &DuplexAction::Gate(GateCmd::Duck));
        assert_eq!(floor.snapshot().floor, Floor::Yielding);
        assert_eq!(
            floor.wire_state(),
            ("both", Some(Lane::Front), "user_speech")
        );
        floor.on_duplex(500, &DuplexAction::Gate(GateCmd::Pause));
        floor.on_duplex(900, &DuplexAction::Gate(GateCmd::Open));
        assert_eq!(floor.snapshot().floor, Floor::FrontSpeaking);

        floor.on_duplex(1_000, &DuplexAction::Gate(GateCmd::Duck));
        assert_eq!(floor.next_wakeup(), Some(8_700));
        assert_eq!(
            floor.next_cmds(8_700),
            vec![FloorCmd::Cut(CutCause::SideTalkTimeout)]
        );
    }

    #[test]
    fn a_yield_nobody_decided_ends_in_a_cut_even_with_nothing_left_to_play() {
        let mut floor = machine();
        floor.offer(0, line(1, 2, "Okay."));
        let (utt, _) = submit(&mut floor, 0).unwrap();
        floor.on_pump(10, PumpEvent::Started { utt, at: 10 });
        floor.on_duplex(100, &DuplexAction::Gate(GateCmd::Pause));
        // The audio ran out while the pump was held, and no verdict came.
        floor.on_pump(200, PumpEvent::SynthDone { utt });
        let at = 300;
        let idle = PumpEvent::Idle {
            utt,
            delivered_ms: 250,
            at,
        };
        floor.on_pump(at, idle);
        assert!(!floor.snapshot().agent_audible);
        // A cut the duplex machine asks for now finds nothing: the actor opens the gate.
        floor.on_duplex(400, &DuplexAction::Cut(CutCause::SideTalkTimeout));
        assert!(!floor.cut_pending());

        // The backstop hands its cut to the actor all the same, which opens the gate.
        floor.offer(1_000, line(2, 2, "Okay."));
        let (utt, _) = submit(&mut floor, 1_000).unwrap();
        floor.on_pump(1_010, PumpEvent::Started { utt, at: 1_010 });
        floor.on_duplex(1_100, &DuplexAction::Gate(GateCmd::Pause));
        floor.on_pump(1_200, PumpEvent::SynthDone { utt });
        let idle = PumpEvent::Idle {
            utt,
            delivered_ms: 190,
            at: 1_300,
        };
        floor.on_pump(1_300, idle);
        assert_eq!(
            floor.next_cmds(1_100 + YIELD_MS),
            vec![FloorCmd::Cut(CutCause::SideTalkTimeout)]
        );
        assert!(floor.cut_pending());
        floor.on_cut(1_100 + YIELD_MS, CutCause::SideTalkTimeout, &[]);
        assert_eq!(floor.snapshot().floor, Floor::Listening);
    }

    #[test]
    fn the_thinking_backstop_follows_an_updated_holding_time() {
        let mut floor = machine();
        floor.offer(0, line(1, 2, "Okay."));
        assert_eq!(floor.next_wakeup(), Some(0));
        // 8,000 + 2,000 ms by default; 4,000 + 2,000 ms after the update.
        let mut updated = machine();
        updated.set_hold_after_silence(4_000);
        updated.offer(0, line(1, 2, "Okay."));
        for (machine, until) in [(&mut floor, 10_000), (&mut updated, 6_000)] {
            machine.on_duplex(1, &DuplexAction::UserSpeaking(true));
            assert_eq!(machine.next_wakeup(), Some(until));
        }
    }

    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % n
        }
    }

    /// Liveness: every state but `Listening` has agent audio playing or a wake-up armed
    /// (`UserSpeaking` has the caller's audio, and ends with the duplex machine's own
    /// timers); and only the newest utterance may be in synthesis.
    fn check(floor: &FloorMachine, seed: u64) {
        let snapshot = floor.snapshot();
        if !matches!(snapshot.floor, Floor::Listening | Floor::UserSpeaking) {
            assert!(
                snapshot.agent_audible || floor.next_wakeup().is_some(),
                "seed {seed}: {:?} with no audio and no wake-up",
                snapshot.floor
            );
        }
        let in_synthesis = floor
            .active
            .iter()
            .filter(|active| !active.synth_done)
            .count();
        assert!(
            in_synthesis <= 1,
            "seed {seed}: {in_synthesis} utterances in synthesis"
        );
        assert!(
            floor
                .active
                .iter()
                .rev()
                .skip(1)
                .all(|active| active.synth_done),
            "seed {seed}: an utterance was submitted behind one still in synthesis"
        );
    }

    #[test]
    fn liveness_over_random_event_sequences() {
        for seed in 1..=400u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            let mut floor = machine();
            let mut now: Ms = 0;
            let mut utts: Vec<UttId> = Vec::new();
            let mut items = 0u64;
            for _ in 0..150 {
                now += if rng.below(12) == 0 {
                    11_000
                } else {
                    rng.below(400)
                };
                let utt = match utts.len() as u64 {
                    0 => UttId(0),
                    // Mostly a recent utterance, sometimes an old (cut or finished) one.
                    n => utts[(n - 1 - rng.below(n.min(3))) as usize],
                };
                match rng.below(16) {
                    0..=3 => {
                        items += 1;
                        let item = if rng.below(3) == 0 {
                            items.saturating_sub(1)
                        } else {
                            items
                        };
                        let utterance = match rng.below(5) {
                            0 => warm(item),
                            1 => sentence(item, 0, "It leaves at nine.", rng.below(2) == 0),
                            2 => Utterance {
                                expires_at: Some(now + 3_000),
                                ..line(item, 7, "Checking.")
                            },
                            _ => line(item, 2 + rng.below(7) as u8, "Sure, let me check."),
                        };
                        floor.offer(now, utterance);
                    }
                    4 | 5 => drop(floor.on_pump(now, PumpEvent::Started { utt, at: now })),
                    // A missing `SynthDone` is the common fault: it is fed half as often.
                    6 => drop(floor.on_pump(now, PumpEvent::SynthDone { utt })),
                    7 | 8 => {
                        floor.on_pump(
                            now,
                            PumpEvent::Idle {
                                utt,
                                delivered_ms: 100,
                                at: now,
                            },
                        );
                    }
                    9 => drop(floor.on_pump(now, PumpEvent::Failed { utt })),
                    10 => {
                        floor.on_duplex(now, &DuplexAction::UserSpeaking(rng.below(2) == 0));
                    }
                    11 => {
                        let gate = [
                            GateCmd::Duck,
                            GateCmd::Pause,
                            GateCmd::Resume,
                            GateCmd::Open,
                        ];
                        floor.on_duplex(now, &DuplexAction::Gate(gate[rng.below(4) as usize]));
                    }
                    12 => drop(floor.on_duplex(now, &DuplexAction::Cut(CutCause::Interrupt))),
                    13 => {
                        floor.on_duplex(now, &end_of_turn(seed, now));
                        if rng.below(2) == 0 {
                            floor.on_settled(now, TurnId(seed));
                        }
                    }
                    14 => drop(floor.drop_item(now, ItemId(items))),
                    _ => drop(floor.tick(now)),
                }
                check(&floor, seed);
                for cmd in floor.next_cmds(now) {
                    match cmd {
                        FloorCmd::Submit { utt, .. } | FloorCmd::Prerender { utt, .. } => {
                            utts.push(utt)
                        }
                        // The actor reports most cuts, not all.
                        FloorCmd::Cut(cause) if rng.below(3) > 0 => {
                            floor.on_cut(now, cause, &[]);
                        }
                        FloorCmd::Cut(_) | FloorCmd::DropWarm(_) => {}
                    }
                }
                check(&floor, seed);
            }

            // No more events but the pump finishing what it plays: the floor comes to rest.
            floor.on_duplex(now, &DuplexAction::UserSpeaking(false));
            floor.on_board(now, &Board::default());
            let mut rounds = 0;
            while floor.snapshot().floor != Floor::Listening || floor.next_wakeup().is_some() {
                rounds += 1;
                assert!(rounds < 2_000, "seed {seed}: the floor never came to rest");
                let audible: Vec<UttId> = floor
                    .active
                    .iter()
                    .filter(|a| a.audible())
                    .map(|a| a.utt)
                    .collect();
                for utt in audible {
                    floor.on_pump(
                        now,
                        PumpEvent::Idle {
                            utt,
                            delivered_ms: 100,
                            at: now,
                        },
                    );
                }
                now = floor.next_wakeup().unwrap_or(now).max(now + 1);
                floor.tick(now);
                floor.next_cmds(now);
                check(&floor, seed);
            }
            assert!(
                floor.active.is_empty() && floor.pending.is_empty(),
                "seed {seed}"
            );
        }
    }
}

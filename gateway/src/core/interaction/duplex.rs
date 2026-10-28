//! The duplex machine: barge-in (duck, pause, classify), the transcript echo filter and the
//! endpoint policy that turns speech-to-text results into ends of turn.
//!
//! Gate commands as this machine issues them: `Duck` lowers the agent (the analyser has
//! already done it when it reports `reflex_ducked`), `Pause` holds it, `Resume` ends a duck
//! or a pause (the pump ramps the level back), and `Open` resets the gate at once when the
//! agent's audio ended during the episode. A cut opens the gate itself (`PumpControl::cut`).
//!
//! On a speech-final result the actor passes the whole segment (`STTResult::turn_transcript`).
//!
//! The speech-to-text hears the agent's echo even when the analyser's echo defence keeps it
//! out of the onsets. While the analyser reports echo, words that come with no onset open a
//! turn that is taken for the agent's own, and a barge-in is judged on the words that are not
//! the agent's.

use std::collections::HashSet;

use super::config::DuplexConfig;
use super::persona::{Lexicon, word_list};
use super::types::{
    Attribution, Class, CutCause, DuplexAction, DuplexEvent, EndOfTurn, EotSource, Floor,
    FloorSnapshot, GateCmd, Ms, TurnId,
};
use crate::core::stt::SttTurnEvent;

/// Smart Turn probabilities: the turn has ended; an eager end of turn may start.
const TURN_END_P: f32 = 0.7;
const EAGER_P: f32 = 0.8;
/// Under this Smart Turn is sure the caller has not finished: a thinking pause, held. In the
/// experience run's sessions (MTR, Candor) a pause under it was at least as likely as the
/// caller's end; from it to 0.7 ends were twice as likely, and holding them cost every such
/// end 800 ms.
const THINKING_P: f32 = 0.05;
/// Less voiced audio than this is noise: not a backchannel, and never uploaded.
const MIN_VOICED_MS: u32 = 250;
/// The silence reports the endpoint and the side-talk rule act on.
const SILENCE_SHORT_MS: u32 = 200;
const SILENCE_SIDE_TALK_MS: u32 = 700;
const SILENCE_CEILING_MS: u32 = 1_500;
/// No final this long after an upload or a claim: the turn ends without one.
const FINAL_DUE_MS: Ms = 3_000;
const HOLD_MS: Ms = 800;
/// Words with no turn this long after a turn of the caller's sound was closed for want of
/// words are that sound's: the turn has its onset. Live (launch demo take 24, 28 Oct): "Yes. Go
/// ahead." to a confirmation came from Deepgram 2.6 s after the caller stopped; the wordless
/// turn had been closed at the silence ceiling, the words opened one with no onset, their
/// attribution was medium, and the yes did not bind ("Just to be sure: ...").
const LATE_WORDS_MS: Ms = FINAL_DUE_MS;
const EAGER_PER_TURN: u8 = 2;
const EAGER_MORE_VOICED_MS: u64 = 300;
/// Echo: this many words, this share of them found in what the agent said in the window (also
/// the measure of a background sentence that repeats its prompt, `guard::repeats_prompt`).
pub(super) const ECHO_MIN_WORDS: usize = 3;
pub(super) const ECHO_SHARE_PERCENT: usize = 60;
const ECHO_WINDOW_MS: Ms = 5_000;
/// This far below the caller's running level is someone in the background.
const BACKGROUND_DB: f32 = 12.0;
const SIDE_TALK_CUT_MS: Ms = 6_000;
const SHORT_ANSWER_WORDS: usize = 3;
const CONTENT_WORDS: usize = 3;
/// How long past `decide_by_ms` the agent stays paused for the front's verdict on words.
const VERDICT_WAIT_MS: Ms = 1_500;

/// The caller's turn from its first sound or word to E.
struct Turn {
    id: TurnId,
    opened_at: Ms,
    /// When the caller began: the first onset the analyser reported and the level of the
    /// speech since (the latest report), or, with no analyser, the first words (no level).
    onset: Option<(Ms, Option<f32>)>,
    acoustic_end: Option<Ms>,
    /// Segments that ended while the turn was held open.
    held: String,
    /// Final pieces of the running segment, and its latest interim.
    finals: String,
    interim: String,
    /// The latest Smart Turn probability since the last onset.
    p: Option<f32>,
    source: EotSource,
    claimed: bool,
    /// The outstanding eager end of turn: its text and the voiced time since.
    eager: Option<(String, u64)>,
    eagers: u8,
    hold_used: bool,
    hold_until: Option<Ms>,
    class: Option<Class>,
    /// `UserSpeaking(true)` was sent.
    speaking: bool,
    /// Its first words came with no onset while the analyser heard the agent's echo.
    echo: bool,
    /// The words the front was last asked about over the agent.
    asked: String,
    /// The caller's voiced audio in this turn.
    voiced_ms: u32,
}

impl Turn {
    /// An eager end of turn on these words.
    fn guess(&mut self, text: String) -> DuplexAction {
        self.eagers += 1;
        self.eager = Some((text.clone(), 0));
        DuplexAction::Eager {
            turn: self.id,
            text,
        }
    }

    /// Words and no sound of the caller's: the agent's own, heard back.
    fn echo_only(&self) -> bool {
        self.echo && self.onset.is_none()
    }

    fn text(&self) -> String {
        [&self.held, &self.finals, &self.interim]
            .into_iter()
            .filter(|part| !part.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// An onset over agent audio, until it is classified.
struct Barge {
    onset: Ms,
    /// The level of its voiced audio so far (the latest report).
    level_db: Option<f32>,
    paused_at: Option<Ms>,
    side_talk: bool,
    voiced_ms: u32,
    offset_seen: bool,
    /// The front model's verdict on the caller's words.
    verdict: Option<Class>,
}

pub struct Duplex {
    cfg: DuplexConfig,
    lexicon: Lexicon,
    request_response: bool,
    last_turn: u64,
    turn: Option<Turn>,
    barge: Option<Barge>,
    /// The caller's running level, from the speech of turns that had words.
    caller_db: Option<f32>,
    /// Since when the caller is voiced.
    voiced_from: Option<Ms>,
    /// Voiced audio since the last flush, and the latest silence report since the last offset.
    voiced_ms: u32,
    silence_ms: u32,
    /// A final is owed by this time (after an upload or a claim).
    final_due: Option<Ms>,
    /// The caller spoke again while an upload was away.
    resumed: bool,
    /// The analyser reports onsets; `false` once it said it cannot.
    analyser: bool,
    /// When the analyser last heard the agent's echo.
    echo_at: Option<Ms>,
    /// A barge-in opens on the caller's first words, not on a bare onset.
    words_first: bool,
    /// The speech-to-text runs its own turn model (Deepgram Flux): its eager end of turn is
    /// the eager signal, and Smart Turn makes none.
    provider_turns: bool,
    /// A claim makes the speech-to-text finish the words it has (Deepgram `Finalize`): a
    /// segment of interim words may be claimed once Smart Turn is sure and the silence the
    /// speech-to-text was asked to end a segment after (its endpointing, ms) has passed.
    claim_after_ms: Option<u32>,
    /// The words of a claim the speech-to-text never answered in time: the turn ended on
    /// them, and the late results that repeat them are not a turn of their own.
    unanswered: Option<String>,
    /// The caller's sound a turn was closed on at the silence ceiling before any of its words
    /// came: its onset, level and when it was closed. Words that come soon after are that
    /// sound's (see [`LATE_WORDS_MS`]).
    unworded: Option<(Ms, Option<f32>, Ms)>,
    /// The caller's answer to a question is expected (the latest input said so).
    answer_expected: bool,
}

impl Duplex {
    /// `request_response`: the speech-to-text returns a transcript only after an upload, so
    /// there are no interim results and the machine asks for the flush itself.
    pub fn new(cfg: DuplexConfig, lexicon: Lexicon, request_response: bool) -> Self {
        Duplex {
            cfg,
            lexicon,
            request_response,
            last_turn: 0,
            turn: None,
            barge: None,
            caller_db: None,
            voiced_from: None,
            voiced_ms: 0,
            silence_ms: 0,
            final_due: None,
            resumed: false,
            analyser: true,
            echo_at: None,
            words_first: false,
            provider_turns: false,
            claim_after_ms: None,
            unanswered: None,
            unworded: None,
            answer_expected: false,
        }
    }

    /// A speech-to-text that finishes its words when the segment is claimed, with its
    /// endpointing silence: see [`Self::claim_after_ms`](Duplex).
    pub fn with_turn_end_on_claim(mut self, after_ms: Option<u32>) -> Self {
        self.claim_after_ms = after_ms;
        self
    }

    /// A speech-to-text with its own turn model: see [`Self::on_turn_event`].
    pub fn with_provider_turns(mut self, on: bool) -> Self {
        self.provider_turns = on;
        self
    }

    /// The settings after an `interaction_update`; they apply from the next event.
    pub fn set_config(&mut self, cfg: DuplexConfig) {
        self.cfg = cfg;
    }

    /// A speech-to-text result (the fields of `STTResult` the machine reads).
    pub fn on_stt(
        &mut self,
        now: Ms,
        text: &str,
        is_final: bool,
        is_speech_final: bool,
        floor: &FloorSnapshot,
        answer_expected: bool,
    ) -> Vec<DuplexAction> {
        self.answer_expected = answer_expected;
        let mut out = Vec::new();
        let text = text.trim();
        if text.is_empty() && (!is_speech_final || self.turn.is_none()) {
            return out;
        }
        // The late answer to a claim the turn already ended on.
        if let Some(stale) = self.unanswered.take()
            && self.turn.is_none()
            && word_list(&stale) == word_list(text)
        {
            if !(is_final || is_speech_final) {
                self.unanswered = Some(stale);
            }
            return out;
        }
        let over_agent = floor.agent_audible && self.cfg.barge_in;
        match &self.turn {
            // The words of a sound the analyser heard and a turn was closed on before they came.
            None if self
                .unworded
                .is_some_and(|(_, _, closed)| now <= closed + LATE_WORDS_MS) =>
            {
                let sound = self.unworded.take();
                self.onset(now, None, false, floor, &mut out);
                if let (Some(turn), Some((at, level, _))) = (&mut self.turn, sound) {
                    turn.onset.get_or_insert((at, level));
                }
            }
            // Words with no onset. While the analyser hears the agent's echo and no sound of
            // the caller's, they are taken for the agent's: the turn opens and nothing is
            // ducked.
            None if self.echoing(now) && self.voiced_from.is_none() => {
                self.open_turn(now).echo = true
            }
            // The analyser is absent or late.
            None => self.onset(now, None, false, floor, &mut out),
            // The first words after an onset that opened no barge-in open it.
            Some(turn)
                if self.words_first
                    && over_agent
                    && self.barge.is_none()
                    && turn.class.is_none()
                    && turn.text().is_empty() =>
            {
                let level_db = turn.onset.and_then(|(_, level)| level);
                self.open_barge(now, level_db);
                out.push(DuplexAction::Gate(GateCmd::Duck));
            }
            Some(_) => {}
        }
        let turn = self.open_turn(now);
        if is_speech_final {
            if !text.is_empty() {
                turn.finals = text.to_owned();
            }
            turn.interim.clear();
            self.barge_step(now, floor, &mut out);
            self.speech_final(now, floor, answer_expected, &mut out);
        } else {
            if is_final {
                if !turn.finals.is_empty() {
                    turn.finals.push(' ');
                }
                turn.finals.push_str(text);
                turn.interim.clear();
            } else {
                turn.interim = text.to_owned();
            }
            // New words merge into a held turn.
            turn.hold_until = None;
            self.interim(now, floor, &mut out);
            self.advance(now, floor, &mut out);
        }
        out
    }

    /// A turn event of a speech-to-text with its own turn model (Deepgram Flux), after the
    /// result that carried it went through [`Self::on_stt`]: its eager end of turn is the eager
    /// signal (under the rules Smart Turn's is), and the caller going on withdraws it.
    pub fn on_turn_event(
        &mut self,
        event: SttTurnEvent,
        floor: &FloorSnapshot,
    ) -> Vec<DuplexAction> {
        let mut out = Vec::new();
        let (cfg, request_response) = (&self.cfg, self.request_response);
        let Some(turn) = &mut self.turn else {
            return out;
        };
        match event {
            SttTurnEvent::EagerEndOfTurn => {
                let text = turn.text();
                if may_guess(cfg, request_response, turn, &text, floor) {
                    out.push(turn.guess(text));
                }
            }
            SttTurnEvent::TurnResumed => {
                if turn.eager.take().is_some() {
                    out.push(DuplexAction::EagerWithdrawn { turn: turn.id });
                }
            }
        }
        out
    }

    /// The speech-to-text failed: the turn in progress ends with no words.
    pub fn on_stt_error(&mut self, now: Ms) -> Vec<DuplexAction> {
        let mut out = Vec::new();
        if self.turn.is_some() || self.final_due.is_some() {
            self.fail_turn(now, &mut out);
        }
        out
    }

    /// The client says the caller's audio has ended (`audio_end`): the turn ends now, not at
    /// the next silence report. The speech-to-text keeps its callbacks, so later turns still
    /// arrive (the mode-off `finalize_stt` reconnects the provider and drops them).
    pub fn on_audio_end(&mut self, now: Ms) -> Vec<DuplexAction> {
        let mut out = Vec::new();
        if self.final_due.is_some() {
            // An upload or a claim is away: its final is E.
        } else if self.request_response {
            self.open_turn(now);
            self.final_due = Some(now + FINAL_DUE_MS);
            self.voiced_ms = 0;
            self.silence_ms = 0;
            out.push(DuplexAction::FinalizeStt { upload: true });
        } else if let Some(turn) = &mut self.turn
            && !turn.claimed
            && !turn.text().is_empty()
        {
            turn.claimed = true;
            self.final_due = Some(now + FINAL_DUE_MS);
            out.push(DuplexAction::ClaimSegment);
        }
        out
    }

    /// A turn the client typed: an end of turn with no audio.
    pub fn on_typed(&mut self, now: Ms, text: &str) -> Vec<DuplexAction> {
        self.last_turn += 1;
        vec![DuplexAction::EndOfTurn(EndOfTurn {
            turn: TurnId(self.last_turn),
            at: now,
            acoustic_end: None,
            onset_at: None,
            text: text.trim().to_owned(),
            source: EotSource::Typed,
            eager_same: None,
            class: Class::Interrupt,
            attribution: Attribution::Medium,
            suspected_echo: false,
            unclear: false,
            foreign: false,
        })]
    }

    pub fn on_event(
        &mut self,
        now: Ms,
        event: DuplexEvent,
        floor: &FloorSnapshot,
        answer_expected: bool,
    ) -> Vec<DuplexAction> {
        self.answer_expected = answer_expected;
        let mut out = Vec::new();
        match event {
            DuplexEvent::Onset {
                at,
                level_db,
                reflex_ducked,
            } => {
                self.unworded = None;
                self.voiced_from = Some(at);
                self.silence_ms = 0;
                // The caller goes on while the claim or the upload is away: its final holds
                // the words so far and the turn ends with the rest (live TP1: a claimed
                // final came 300 ms after the caller spoke again and ended the turn there).
                self.resumed |= self.final_due.is_some();
                self.onset(at, Some(level_db), reflex_ducked, floor, &mut out);
            }
            DuplexEvent::Level { level_db } => {
                if let Some(barge) = &mut self.barge {
                    barge.level_db = Some(level_db);
                }
                if let Some((_, level)) = self.turn.as_mut().and_then(|turn| turn.onset.as_mut()) {
                    *level = Some(level_db);
                }
            }
            DuplexEvent::Offset { at, voiced_ms } => {
                self.voiced_from = None;
                self.silence_ms = 0;
                self.voiced_ms = self.voiced_ms.saturating_add(voiced_ms);
                if let Some(turn) = &mut self.turn {
                    turn.acoustic_end = Some(at);
                    turn.voiced_ms = turn.voiced_ms.saturating_add(voiced_ms);
                    // A new pause: it may be held, whatever a final that came while the
                    // caller spoke did.
                    turn.hold_used = false;
                    if let Some((_, more)) = &mut turn.eager {
                        *more += u64::from(voiced_ms);
                    }
                }
                if let Some(barge) = &mut self.barge {
                    barge.voiced_ms = barge.voiced_ms.saturating_add(voiced_ms);
                    barge.offset_seen = true;
                }
            }
            DuplexEvent::Silence { ms, .. } => self.silence_ms = ms,
            DuplexEvent::SmartTurn { p, .. } => {
                let lexicon = &self.lexicon;
                if let Some(turn) = &mut self.turn {
                    turn.p = Some(p);
                    // A turn held for want of a verdict: a sure one ends it now, and so does
                    // one that does not hold it (not a thinking pause, no dangling word).
                    // A dangling word holds whatever the verdict (see `speech_final`).
                    let holds = p < THINKING_P || dangles(lexicon, &turn.held);
                    if !holds && let Some(until) = &mut turn.hold_until {
                        *until = (*until).min(now);
                    }
                }
                // A pause in a long turn that Smart Turn says is not finished: the moment for a
                // listening sound. Never over the agent, never while an answer is expected.
                let long = |turn: &Turn| turn.voiced_ms >= MIN_VOICED_MS;
                let listening = !floor.agent_audible
                    && !answer_expected
                    && self.barge.is_none()
                    && self.final_due.is_none()
                    && self.voiced_from.is_none();
                if let Some(turn) = self.turn.as_ref().filter(|turn| !turn.echo && long(turn))
                    && p < TURN_END_P
                    && listening
                {
                    out.push(DuplexAction::Backchannel {
                        text: turn.text(),
                        voiced_ms: turn.voiced_ms,
                    });
                }
            }
            DuplexEvent::Echo { at } => self.echo_at = Some(at),
            DuplexEvent::WordsFirst(on) => self.words_first = on,
            DuplexEvent::Degraded(_) => {
                // The analyser stopped: no offset or silence report will end what it began,
                // and from now on the first words of a turn are its onset.
                self.analyser = false;
                self.voiced_from = None;
                self.silence_ms = 0;
                let wordless = |turn: &Turn| turn.text().is_empty();
                if self.final_due.is_none() && self.turn.as_ref().is_some_and(wordless) {
                    self.close_turn(now, floor, &mut out);
                }
            }
        }
        self.advance(now, floor, &mut out);
        out
    }

    /// `answer_expected`: the endpoint ends an answer to a question at its first pause.
    pub fn tick(
        &mut self,
        now: Ms,
        floor: &FloorSnapshot,
        answer_expected: bool,
    ) -> Vec<DuplexAction> {
        self.answer_expected = answer_expected;
        let mut out = Vec::new();
        self.advance(now, floor, &mut out);
        out
    }

    /// The next time `tick(now, ..)` has something to do.
    pub fn next_wakeup(&self) -> Option<Ms> {
        let barge = self.barge.as_ref().map(|barge| match barge.paused_at {
            Some(paused_at) if barge.side_talk => self.side_talk_cut_at(barge.onset, paused_at),
            Some(_) => barge.onset + Ms::from(self.cfg.decide_by_ms),
            None => barge.onset + Ms::from(self.cfg.pause_after_ms.min(self.cfg.decide_by_ms)),
        });
        let turn = self.turn.as_ref();
        let eager = turn.and_then(|turn| {
            let (_, more) = turn.eager.as_ref()?;
            Some(self.voiced_from? + EAGER_MORE_VOICED_MS.saturating_sub(*more))
        });
        [
            barge,
            self.final_due,
            turn.and_then(|turn| turn.hold_until),
            eager,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    fn open_turn(&mut self, now: Ms) -> &mut Turn {
        let last_turn = &mut self.last_turn;
        self.turn.get_or_insert_with(|| {
            *last_turn += 1;
            Turn {
                id: TurnId(*last_turn),
                opened_at: now,
                onset: None,
                acoustic_end: None,
                held: String::new(),
                finals: String::new(),
                interim: String::new(),
                p: None,
                source: EotSource::SttSpeechFinal,
                claimed: false,
                eager: None,
                eagers: 0,
                hold_used: false,
                hold_until: None,
                class: None,
                speaking: false,
                echo: false,
                asked: String::new(),
                voiced_ms: 0,
            }
        })
    }

    /// Whether the analyser heard the agent's echo a moment ago.
    /// Words that arrive within `duplex.echo.tail_ms` of it may still be its words.
    fn echoing(&self, now: Ms) -> bool {
        self.echo_at
            .is_some_and(|at| now.saturating_sub(at) <= Ms::from(self.cfg.echo.tail_ms))
    }

    /// The words of the turn so far that a barge-in is judged on. While the analyser hears
    /// echo the transcript holds the agent's words too, and an onset that reached this machine
    /// passed the echo defence: only the words that are not the agent's count.
    fn caller_words(&self, now: Ms, floor: &FloorSnapshot) -> Vec<String> {
        let mut words = word_list(&self.turn.as_ref().map(Turn::text).unwrap_or_default());
        if self.echoing(now) {
            let agent = agent_words(floor, now.saturating_sub(ECHO_WINDOW_MS));
            words.retain(|word| !agent.contains(word));
        }
        words
    }

    fn open_barge(&mut self, at: Ms, level_db: Option<f32>) {
        self.barge = Some(Barge {
            onset: at,
            level_db,
            paused_at: None,
            side_talk: false,
            voiced_ms: 0,
            offset_seen: false,
            verdict: None,
        });
    }

    /// The caller starts to speak: over agent audio that opens a barge-in, otherwise the
    /// caller simply has the floor.
    fn onset(
        &mut self,
        at: Ms,
        level_db: Option<f32>,
        reflex_ducked: bool,
        floor: &FloorSnapshot,
        out: &mut Vec<DuplexAction>,
    ) {
        let barging = self.barge.is_some();
        let open_barge = floor.agent_audible && self.cfg.barge_in;
        // On a phone line with no echo canceller a sound over the agent may be the agent's
        // own: it opens the turn, and the first words open the barge-in.
        let wait_for_words = open_barge && self.words_first && level_db.is_some();
        let heard = level_db.is_some() || !self.analyser;
        let turn = self.open_turn(at);
        if turn.onset.is_none() && heard {
            turn.onset = Some((at, level_db));
        }
        // New speech merges into a held turn, and Smart Turn has to judge it again: its next
        // pause may be held too.
        turn.p = None;
        turn.hold_until = None;
        turn.hold_used = false;
        if wait_for_words {
            // Nothing is ducked yet: `on_stt` opens the barge-in.
        } else if open_barge {
            if !barging {
                self.open_barge(at, level_db);
                if !reflex_ducked {
                    out.push(DuplexAction::Gate(GateCmd::Duck));
                }
            }
        } else {
            if reflex_ducked && !barging {
                // The analyser ducked by itself and no barge-in opens (the agent's last
                // words were already delivered, or barge-in was switched off): nothing
                // else would give the level back, and every later line would play ducked.
                out.push(DuplexAction::Gate(GateCmd::Resume));
            }
            if !turn.speaking {
                turn.speaking = true;
                out.push(DuplexAction::UserSpeaking(true));
            }
        }
    }

    fn advance(&mut self, now: Ms, floor: &FloorSnapshot, out: &mut Vec<DuplexAction>) {
        self.barge_step(now, floor, out);
        self.endpoint_step(now, floor, out);
        self.eager_step(now, floor, out);
        if self.final_due.is_some_and(|due| due <= now) {
            match self.turn.as_ref().map(Turn::text) {
                // A claim the speech-to-text never answered: its words are the turn.
                Some(text) if !self.request_response && !text.is_empty() => {
                    self.unanswered = Some(text.clone());
                    self.end_turn(now, text, false, floor, out);
                }
                _ => self.fail_turn(now, out),
            }
        }
        if let Some(turn) = &self.turn
            && turn.hold_until.is_some_and(|until| until <= now)
        {
            let text = turn.held.clone();
            self.end_turn(now, text, false, floor, out);
        }
    }

    /// The barge-in machine: `Idle -> Ducked -> Paused -> Idle`.
    fn barge_step(&mut self, now: Ms, floor: &FloorSnapshot, out: &mut Vec<DuplexAction>) {
        let Some(barge) = &self.barge else {
            return;
        };
        let voiced = self.voiced_from.is_some();
        if barge.side_talk {
            let paused_at = barge.paused_at.unwrap_or(barge.onset);
            if self.silence_ms >= SILENCE_SIDE_TALK_MS {
                self.barge = None;
                out.push(DuplexAction::Gate(GateCmd::Resume));
            } else if self.side_talk_cut_at(barge.onset, paused_at) <= now {
                self.barge = None;
                out.push(DuplexAction::Cut(CutCause::SideTalkTimeout));
                self.caller_has_the_floor(voiced, out);
            }
            return;
        }
        if !floor.agent_audible {
            // The agent finished or was cut meanwhile: nothing is left to yield.
            self.barge = None;
            out.push(DuplexAction::Gate(GateCmd::Open));
            self.caller_has_the_floor(voiced, out);
            return;
        }
        let words = self.caller_words(now, floor);
        let class = self.classify(barge, &words, voiced, now, floor);
        let pause_due =
            barge.paused_at.is_none() && barge.onset + Ms::from(self.cfg.pause_after_ms) <= now;
        match class {
            Some(class) => self.decide(now, class, out),
            None if pause_due => {
                if let Some(barge) = &mut self.barge {
                    barge.paused_at = Some(now);
                }
                out.push(DuplexAction::Gate(GateCmd::Pause));
            }
            None => {}
        }
    }

    /// The classifier, first match. What the caller's words mean (a stop, a request, a
    /// continuer, side talk) is the front model's verdict on the interim results (chapter 9):
    /// the rest is sound. Rows that need words never fire on request-response
    /// speech-to-text, whose words come with the end of the turn.
    fn classify(
        &self,
        barge: &Barge,
        words: &[String],
        voiced: bool,
        now: Ms,
        floor: &FloorSnapshot,
    ) -> Option<Class> {
        let ended = barge.offset_seen && !voiced;
        let since = now.saturating_sub(ECHO_WINDOW_MS);
        if !words.is_empty() && is_echo(words, floor, since) {
            return Some(Class::Echo);
        }
        if let Some(verdict) = barge.verdict {
            return Some(verdict);
        }
        // The level is judged where the agent would pause, or at the end of a shorter sound:
        // by then it is the level of the speech, not of the 64 ms of its onset (the first
        // sound of a word can be 12 dB below the rest of it).
        let decided = barge.onset + Ms::from(self.cfg.decide_by_ms) <= now;
        let heard = ended || decided || barge.onset + Ms::from(self.cfg.pause_after_ms) <= now;
        if heard
            && let (Some(level), Some(caller)) = (barge.level_db, self.caller_db)
            && caller - level >= BACKGROUND_DB
        {
            return Some(Class::Background);
        }
        if words.is_empty() && ended {
            if barge.voiced_ms < MIN_VOICED_MS {
                return Some(Class::Noise);
            }
            if !decided {
                return Some(Class::Backchannel);
            }
        }
        // Words over the agent and no verdict yet: the agent stays paused while the front
        // reads them, up to its budget.
        let reading = !words.is_empty()
            && !voiced
            && now < barge.onset + Ms::from(self.cfg.decide_by_ms) + VERDICT_WAIT_MS;
        (decided && !reading).then_some(if voiced {
            Class::Interrupt
        } else {
            Class::Noise
        })
    }

    /// The front model's verdict on the caller's words over the agent (an interim result, or
    /// the end of the turn): a stop or a request takes the floor, a continuer lets the agent
    /// go on, side talk pauses it. A verdict for a turn whose barge-in was already judged a
    /// listening sound or noise still takes the floor from an agent that speaks: a short
    /// "stop" is over before its words are there.
    pub fn front_verdict(
        &mut self,
        now: Ms,
        turn: TurnId,
        class: Class,
        floor: &FloorSnapshot,
    ) -> Vec<DuplexAction> {
        let mut out = Vec::new();
        if self.turn.as_ref().is_none_or(|open| open.id != turn) {
            return out;
        }
        if let Some(barge) = &mut self.barge {
            if !barge.side_talk {
                barge.verdict = Some(class);
            }
            self.advance(now, floor, &mut out);
            return out;
        }
        let went_on = |open: &Turn| matches!(open.class, Some(Class::Backchannel | Class::Noise));
        if class == Class::Interrupt
            && floor.agent_audible
            && self.cfg.barge_in
            && self.turn.as_ref().is_some_and(went_on)
        {
            out.push(DuplexAction::Classified(Class::Interrupt));
            if let Some(open) = &mut self.turn {
                open.class = Some(Class::Interrupt);
            }
            out.push(DuplexAction::Cut(CutCause::Interrupt));
            self.caller_has_the_floor(true, &mut out);
        }
        out
    }

    /// The caller's words over the agent changed: the front is asked about them (the newest
    /// interim cancels the request before it). Only while they may still take the floor.
    fn interim(&mut self, now: Ms, floor: &FloorSnapshot, out: &mut Vec<DuplexAction>) {
        if !(floor.agent_audible && self.cfg.barge_in) {
            return;
        }
        let words = self.caller_words(now, floor);
        // The agent's own words heard back are nobody's to decide.
        if is_echo(&words, floor, now.saturating_sub(ECHO_WINDOW_MS)) {
            return;
        }
        let Some(turn) = &mut self.turn else {
            return;
        };
        let open =
            self.barge.is_some() || matches!(turn.class, Some(Class::Backchannel | Class::Noise));
        let text = words.join(" ");
        if !open || words.is_empty() || turn.asked == text {
            return;
        }
        turn.asked = text.clone();
        out.push(DuplexAction::Interim {
            turn: turn.id,
            text,
        });
    }

    fn decide(&mut self, now: Ms, class: Class, out: &mut Vec<DuplexAction>) {
        out.push(DuplexAction::Classified(class));
        if let Some(turn) = &mut self.turn {
            turn.class = Some(class);
        }
        let paused = self
            .barge
            .as_ref()
            .is_some_and(|barge| barge.paused_at.is_some());
        match class {
            Class::Interrupt => {
                self.barge = None;
                out.push(DuplexAction::Cut(CutCause::Interrupt));
                self.caller_has_the_floor(true, out);
            }
            Class::TalkingToOthers => {
                if let Some(barge) = &mut self.barge {
                    barge.side_talk = true;
                    barge.paused_at.get_or_insert(now);
                }
                if !paused {
                    out.push(DuplexAction::Gate(GateCmd::Pause));
                }
            }
            Class::Backchannel | Class::Background | Class::Noise | Class::Echo => {
                self.barge = None;
                out.push(DuplexAction::Gate(GateCmd::Resume));
            }
        }
    }

    fn caller_has_the_floor(&mut self, speaking: bool, out: &mut Vec<DuplexAction>) {
        if let Some(turn) = &mut self.turn
            && speaking
            && !turn.speaking
        {
            turn.speaking = true;
            out.push(DuplexAction::UserSpeaking(true));
        }
    }

    /// A side-talk pause ends at the maximum pause, and at the latest 6 s after the onset.
    fn side_talk_cut_at(&self, onset: Ms, paused_at: Ms) -> Ms {
        (onset + SIDE_TALK_CUT_MS).min(paused_at + Ms::from(self.cfg.max_pause_ms))
    }

    /// Eager end of turn: Smart Turn is sure before the final transcript.
    fn eager_step(&mut self, now: Ms, floor: &FloorSnapshot, out: &mut Vec<DuplexAction>) {
        let voiced_from = self.voiced_from;
        let Some(turn) = &mut self.turn else {
            return;
        };
        let text = turn.text();
        if let Some((eager, more)) = &turn.eager {
            let more = more + voiced_from.map_or(0, |from| now.saturating_sub(from));
            if word_list(eager) != word_list(&text) || more >= EAGER_MORE_VOICED_MS {
                turn.eager = None;
                out.push(DuplexAction::EagerWithdrawn { turn: turn.id });
            }
        }
        // A speech-to-text with its own turn model makes the guesses itself.
        if !self.provider_turns
            && turn.p.is_some_and(|p| p >= EAGER_P)
            && voiced_from.is_none()
            && may_guess(&self.cfg, self.request_response, turn, &text, floor)
        {
            out.push(turn.guess(text));
        }
    }

    /// The endpoint policy: it only makes E arrive sooner (a claim, an upload) or drops
    /// audio that holds no speech. While the answer to a question is expected, a pause ends
    /// the turn once Smart Turn has judged it, unless it is sure the caller has not finished
    /// (live MA2: "Window, please." had p 0.49 and waited 1.3 s for the silence ceiling, past
    /// the question's 5 s deadline).
    fn endpoint_step(&mut self, now: Ms, floor: &FloorSnapshot, out: &mut Vec<DuplexAction>) {
        let holding = self
            .turn
            .as_ref()
            .is_some_and(|turn| turn.hold_until.is_some());
        if self.final_due.is_some() || self.voiced_from.is_some() || holding {
            return;
        }
        let end_p = if self.answer_expected {
            THINKING_P
        } else {
            TURN_END_P
        };
        let sure = self
            .turn
            .as_ref()
            .is_some_and(|turn| turn.p.is_some_and(|p| p >= end_p));
        let ceiling = self.silence_ms >= SILENCE_CEILING_MS;
        // Words that dangle are not claimed early: the caller is mid-phrase whatever Smart
        // Turn heard (the silence ceiling still claims them).
        let dangling =
            (self.turn.as_ref()).is_some_and(|turn| dangles(&self.lexicon, &turn.text()));
        let early = self.silence_ms >= SILENCE_SHORT_MS && sure && !dangling;
        let source = if ceiling {
            EotSource::SilenceCeiling
        } else {
            EotSource::SmartTurn
        };
        if self.request_response {
            if self.voiced_ms >= MIN_VOICED_MS && (ceiling || early) {
                self.open_turn(now).source = source;
                self.final_due = Some(now + FINAL_DUE_MS);
                self.voiced_ms = 0;
                self.silence_ms = 0;
                out.push(DuplexAction::FinalizeStt { upload: true });
            } else if ceiling {
                // Noise and silence are never uploaded, and the buffer does not grow.
                self.voiced_ms = 0;
                self.silence_ms = 0;
                out.push(DuplexAction::FinalizeStt { upload: false });
                self.close_turn(now, floor, out);
            }
        } else if let Some(turn) = &mut self.turn
            && !turn.claimed
        {
            // The agent's echo is silence to the analyser, so the silence reports keep
            // coming while the speech-to-text hears it: its words are left to the vendor's
            // own end of the segment.
            let tail = Ms::from(self.cfg.echo.tail_ms);
            let echo = turn.echo_only()
                && self
                    .echo_at
                    .is_some_and(|at| now.saturating_sub(at) <= tail);
            // Interim words alone are claimed early only where the claim makes the
            // speech-to-text finish them and its own endpointing should have ended the
            // segment by now; a provider with its own turn model ends its turns.
            let words = !turn.text().is_empty();
            let finishes = self
                .claim_after_ms
                .is_some_and(|after| self.silence_ms >= after)
                && !self.provider_turns
                && words;
            let due = (early && (!turn.finals.is_empty() || finishes)) || (ceiling && words);
            if echo {
                // Nothing to claim.
            } else if due {
                turn.claimed = true;
                turn.source = source;
                self.final_due = Some(now + FINAL_DUE_MS);
                out.push(DuplexAction::ClaimSegment);
            } else if ceiling {
                self.silence_ms = 0;
                self.close_turn(now, floor, out);
            }
        }
    }

    /// A speech-final result: E, unless the pause it came in is not over. The turn is held
    /// (once per pause, at most `HOLD_MS`) when the words dangle, and,
    /// where the speech-to-text ended the segment by its endpointing (no claim, no upload, no
    /// turn model of its own) and the analyser heard the turn, also
    /// when Smart Turn is sure the caller has not finished (`THINKING_P`), when its verdict on
    /// this pause is not in yet, or when the caller already speaks again, in a turn that did not
    /// begin over the agent. A verdict that would
    /// not have held the turn ends the hold at once (`on_event`). Not while the answer to a
    /// question is expected: a "yes" binds at once.
    fn speech_final(
        &mut self,
        now: Ms,
        floor: &FloorSnapshot,
        answer_expected: bool,
        out: &mut Vec<DuplexAction>,
    ) {
        self.final_due = None;
        let resumed = std::mem::take(&mut self.resumed);
        let Some(turn) = &mut self.turn else {
            return;
        };
        let text = turn.text();
        if resumed {
            // The caller went on while the upload or the claim was away: these words are
            // kept, and the turn ends with the rest, at the next flush, claim or final.
            turn.held = text;
            turn.finals.clear();
            turn.claimed = false;
            return self.endpoint_step(now, floor, out);
        }
        let unsure = turn.p.is_some_and(|p| p < TURN_END_P);
        let dangling = dangles(&self.lexicon, &text);
        // A thinking pause: Smart Turn is sure the caller has not finished, or its verdict is
        // not in (the final came first, or the caller went on before it). A speech-to-text with
        // its own turn model (Flux) has judged the pause itself. Only in a turn the caller
        // began in the quiet: one that began over the agent (an interruption, a continuer, side
        // talk) wants its answer now, and a pause in it is not a thinking pause (the hold cost
        // FDB's interruption and backchannel turns 0.2 to 1.6 s).
        let judged = !self.request_response
            && !self.provider_turns
            && !turn.claimed
            && self.analyser
            && turn.onset.is_some()
            && turn.class.is_none()
            && !turn.echo;
        let thinking = judged && !answer_expected && turn.p.is_none_or(|p| p < THINKING_P);
        // A caller who went on after a held pause tells a story: an unsure verdict holds the
        // next pause too (live TP1: "... warm by the sea." was held at p 0.04, then "... where
        // the kids can swim and play." ended the turn at p 0.57 and the rest became a second
        // task, three runs of three).
        let story = judged && !answer_expected && unsure && !turn.held.is_empty();
        // A last word that dangles ("... the download numbers in the") holds the turn whatever
        // Smart Turn says: it hears sound, not words. Live (launch demo take 22 and 24, 28
        // Oct): it was sure at a 200 ms gap the analyser heard in "... in the terminal?", the
        // claim made the speech-to-text end its segment at "the", and "terminal?" became a
        // turn of its own (read as the same request again, "change it, or a second one?").
        if !turn.hold_used && (dangling || thinking || story) {
            turn.hold_used = true;
            turn.hold_until = Some(now + HOLD_MS);
            turn.held = text;
            turn.finals.clear();
            turn.claimed = false;
            self.silence_ms = 0;
        } else {
            self.end_turn(now, text, false, floor, out);
        }
    }

    /// The turn ends with no final to wait for: silence, noise, or a held turn nobody
    /// continued.
    fn close_turn(&mut self, now: Ms, floor: &FloorSnapshot, out: &mut Vec<DuplexAction>) {
        match &self.turn {
            Some(turn) if !turn.held.is_empty() => {
                let text = turn.held.clone();
                self.end_turn(now, text, false, floor, out);
            }
            Some(turn) => {
                if turn.speaking {
                    out.push(DuplexAction::UserSpeaking(false));
                }
                if let Some((at, level)) = turn.onset.filter(|_| turn.voiced_ms > 0) {
                    self.unworded = Some((at, level, now));
                }
                self.turn = None;
            }
            None => {}
        }
    }

    fn fail_turn(&mut self, now: Ms, out: &mut Vec<DuplexAction>) {
        let floor = FloorSnapshot {
            floor: Floor::Listening,
            agent_audible: false,
            playing: None,
            recent_agent_lines: Vec::new(),
        };
        // The words of an earlier upload of this turn are not lost with a later one.
        let held = self.turn.as_ref().map(|turn| turn.held.clone());
        let held = held.unwrap_or_default();
        let failed = held.is_empty();
        self.end_turn(now, held, failed, &floor, out);
    }

    fn end_turn(
        &mut self,
        now: Ms,
        text: String,
        failed: bool,
        floor: &FloorSnapshot,
        out: &mut Vec<DuplexAction>,
    ) {
        self.final_due = None;
        self.resumed = false;
        self.open_turn(now);
        let Some(turn) = self.turn.take() else {
            return;
        };
        let since = turn.onset.map_or(turn.opened_at, |(at, _)| at);
        let suspected_echo = turn.echo_only()
            || is_echo(
                &word_list(&text),
                floor,
                since.saturating_sub(ECHO_WINDOW_MS),
            );
        let level = turn.onset.and_then(|(_, level)| level);
        let near = match (level, self.caller_db) {
            (Some(level), Some(caller)) => caller - level < BACKGROUND_DB,
            _ => true,
        };
        let attribution = if suspected_echo {
            Attribution::Low
        } else if turn.onset.is_some() && near {
            Attribution::High
        } else {
            Attribution::Medium
        };
        if let Some(level) = level
            && !suspected_echo
            && !text.is_empty()
        {
            self.caller_db = Some(
                self.caller_db
                    .map_or(level, |caller| 0.8 * caller + 0.2 * level),
            );
        }
        // A barge-in that is undecided when the turn ends: the words settle it, the agent
        // goes on. Not while the caller speaks over the agent's echo: the segment that ended
        // was the echo's, and the sound that opened the barge-in passed the echo defence.
        let over_echo = self.echoing(now) && self.voiced_from.is_some();
        if self.barge.as_ref().is_some_and(|barge| !barge.side_talk) && !over_echo {
            self.decide(now, Class::Noise, out);
        }
        if turn.speaking {
            out.push(DuplexAction::UserSpeaking(false));
        }
        out.push(DuplexAction::EndOfTurn(EndOfTurn {
            turn: turn.id,
            at: now,
            acoustic_end: turn.acoustic_end,
            onset_at: turn.onset.map(|(at, _)| at),
            eager_same: turn
                .eager
                .as_ref()
                .map(|(eager, _)| word_list(eager) == word_list(&text)),
            source: if failed {
                EotSource::SttFailed
            } else {
                turn.source
            },
            class: if suspected_echo {
                Class::Echo
            } else {
                turn.class.unwrap_or(Class::Interrupt)
            },
            attribution,
            suspected_echo,
            text,
            unclear: false,
            foreign: false,
        }));
    }
}

/// The words end on one that leaves the sentence open ("and", "to", "the").
fn dangles(lexicon: &Lexicon, text: &str) -> bool {
    word_list(text)
        .last()
        .is_some_and(|last| lexicon.dangling_end_words.contains(last))
}

/// Whether an eager end of turn may be guessed for `turn` (whose words are `text`) now:
/// eager mode is on, the turn has words and no guess outstanding, fewer than two so far, it is
/// not claimed, held or the agent's echo. Request-response speech-to-text has no words before
/// the final, only held ones.
fn may_guess(
    cfg: &DuplexConfig,
    request_response: bool,
    turn: &Turn,
    text: &str,
    floor: &FloorSnapshot,
) -> bool {
    let since = turn.onset.map_or(turn.opened_at, |(at, _)| at);
    cfg.eager
        && !request_response
        && turn.eager.is_none()
        && turn.eagers < EAGER_PER_TURN
        && !text.is_empty()
        && !turn.claimed
        && turn.hold_until.is_none()
        && !turn.echo_only()
        && !is_echo(
            &word_list(text),
            floor,
            since.saturating_sub(ECHO_WINDOW_MS),
        )
}

/// The words of the agent's lines whose playout ended at or after `since`.
fn agent_words(floor: &FloorSnapshot, since: Ms) -> HashSet<String> {
    floor
        .recent_agent_lines
        .iter()
        .filter(|(ended, _)| *ended >= since)
        .flat_map(|(_, line)| word_list(line))
        .collect()
}

/// The transcript echo filter: most of the words are the agent's own, from lines whose
/// playout ended at or after `since`.
fn is_echo(words: &[String], floor: &FloorSnapshot, since: Ms) -> bool {
    if words.len() < ECHO_MIN_WORDS {
        return false;
    }
    let agent = agent_words(floor, since);
    let found = words.iter().filter(|word| agent.contains(*word)).count();
    found * 100 >= words.len() * ECHO_SHARE_PERCENT
}

#[cfg(test)]
mod tests {
    use super::*;
    use DuplexAction::{Classified, Cut, Gate, UserSpeaking};

    const AGENT_LINE: &str = "I found two flights under four hundred dollars.";

    fn list(phrases: &[&str]) -> Vec<String> {
        phrases.iter().map(|phrase| (*phrase).to_owned()).collect()
    }

    fn lexicon() -> Lexicon {
        Lexicon {
            continuers: list(&["mm-hm", "uh huh", "okay", "yeah", "right"]),
            stop_talking_cues: list(&["stop", "hold on"]),
            side_talk_cues: list(&["honey", "not you"]),
            content_word_stop_list: list(&["the", "a", "to", "i", "you", "it", "is", "and", "um"]),
            dangling_end_words: list(&["and", "to", "the"]),
            strong_accept: list(&["yes", "go ahead"]),
            weak_accept: list(&["okay"]),
            decline: list(&["no"]),
            cancel: list(&["never mind"]),
            ..Lexicon::default()
        }
    }

    fn streaming() -> Duplex {
        Duplex::new(DuplexConfig::default(), lexicon(), false)
    }

    fn uploading() -> Duplex {
        Duplex::new(DuplexConfig::default(), lexicon(), true)
    }

    fn quiet() -> FloorSnapshot {
        FloorSnapshot {
            floor: Floor::Listening,
            agent_audible: false,
            playing: None,
            recent_agent_lines: Vec::new(),
        }
    }

    /// The agent is saying `AGENT_LINE`; a line still playing ends "now".
    fn agent_speaking(now: Ms) -> FloorSnapshot {
        FloorSnapshot {
            floor: Floor::MainSpeaking,
            agent_audible: true,
            playing: None,
            recent_agent_lines: vec![(now, AGENT_LINE.to_owned())],
        }
    }

    fn onset(at: Ms) -> DuplexEvent {
        DuplexEvent::Onset {
            at,
            level_db: -30.0,
            reflex_ducked: true,
        }
    }

    /// An onset with no agent audio delivered: the analyser had nothing to duck.
    fn onset_alone(at: Ms) -> DuplexEvent {
        DuplexEvent::Onset {
            at,
            level_db: -30.0,
            reflex_ducked: false,
        }
    }

    fn classified(class: Class) -> DuplexAction {
        Classified(class)
    }

    /// Smart Turn's verdict that the caller has finished, as it comes in a pause.
    fn sure(duplex: &mut Duplex, at: Ms) -> Vec<DuplexAction> {
        duplex.on_event(at, DuplexEvent::SmartTurn { at, p: 0.9 }, &quiet(), false)
    }

    fn interrupt() -> Vec<DuplexAction> {
        vec![
            classified(Class::Interrupt),
            Cut(CutCause::Interrupt),
            UserSpeaking(true),
        ]
    }

    fn end_of_turn(actions: &[DuplexAction]) -> &EndOfTurn {
        actions
            .iter()
            .find_map(|action| match action {
                DuplexAction::EndOfTurn(turn) => Some(turn),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no end of turn in {actions:?}"))
    }

    /// An onset over the agent at 1,000 ms, then an interim with these words at 1,200 ms.
    /// The front's verdict on the caller's words over the agent (chapter 9).
    fn verdict(duplex: &mut Duplex, now: Ms, class: Class) -> Vec<DuplexAction> {
        duplex.front_verdict(now, TurnId(1), class, &agent_speaking(now))
    }

    /// The front is asked about these words over the agent.
    fn asked(turn: u64, text: &str) -> DuplexAction {
        DuplexAction::Interim {
            turn: TurnId(turn),
            text: text.to_owned(),
        }
    }

    fn barge_with_words(
        duplex: &mut Duplex,
        text: &str,
        answer_expected: bool,
    ) -> Vec<DuplexAction> {
        let floor = agent_speaking(1_000);
        assert_eq!(
            duplex.on_event(1_000, onset(1_000), &floor, answer_expected),
            vec![]
        );
        duplex.on_stt(
            1_200,
            text,
            false,
            false,
            &agent_speaking(1_200),
            answer_expected,
        )
    }

    /// A caller who has spoken `voiced_ms` and pauses, with Smart Turn's `p` at the pause.
    fn paused(
        duplex: &mut Duplex,
        voiced_ms: u32,
        p: f32,
        floor: &FloorSnapshot,
        expected: bool,
    ) -> Vec<DuplexAction> {
        duplex.on_event(0, onset_alone(0), floor, expected);
        duplex.on_stt(
            400,
            "I'm planning a trip next spring and",
            false,
            false,
            floor,
            expected,
        );
        let offset = DuplexEvent::Offset {
            at: Ms::from(voiced_ms),
            voiced_ms,
        };
        duplex.on_event(Ms::from(voiced_ms), offset, floor, expected);
        let at = Ms::from(voiced_ms) + 200;
        duplex.on_event(at, DuplexEvent::Silence { at, ms: 200 }, floor, expected);
        duplex.on_event(at, DuplexEvent::SmartTurn { at, p }, floor, expected)
    }

    fn listens(actions: &[DuplexAction]) -> bool {
        actions
            .iter()
            .any(|action| matches!(action, DuplexAction::Backchannel { .. }))
    }

    #[test]
    fn a_pause_in_a_long_unfinished_turn_is_the_moment_for_a_listening_sound() {
        let actions = paused(&mut streaming(), 1_200, 0.3, &quiet(), false);
        let Some(DuplexAction::Backchannel { text, voiced_ms }) = actions
            .iter()
            .find(|action| matches!(action, DuplexAction::Backchannel { .. }))
        else {
            panic!("a listening sound is due: {actions:?}");
        };
        assert_eq!(text, "I'm planning a trip next spring and");
        assert_eq!(*voiced_ms, 1_200);
        // Not after a start that is noise (the actor holds each language and intensity to its
        // own minimum), not when Smart Turn says the turn is over, not while the agent speaks,
        // not while an answer to a question is expected.
        assert!(!listens(&paused(
            &mut streaming(),
            240,
            0.3,
            &quiet(),
            false
        )));
        assert!(!listens(&paused(
            &mut streaming(),
            1_200,
            0.9,
            &quiet(),
            false
        )));
        assert!(!listens(&paused(
            &mut streaming(),
            1_200,
            0.3,
            &agent_speaking(0),
            false
        )));
        assert!(!listens(&paused(
            &mut streaming(),
            1_200,
            0.3,
            &quiet(),
            true
        )));
    }

    #[test]
    fn the_timeline_ducks_pauses_and_decides() {
        let mut duplex = streaming();
        let floor = agent_speaking(1_000);
        // The analyser did not duck by reflex, so the machine does.
        let event = DuplexEvent::Onset {
            at: 1_000,
            level_db: -30.0,
            reflex_ducked: false,
        };
        assert_eq!(
            duplex.on_event(1_000, event, &floor, false),
            vec![Gate(GateCmd::Duck)]
        );
        assert_eq!(duplex.next_wakeup(), Some(1_400));
        assert_eq!(duplex.tick(1_399, &floor, false), vec![]);
        assert_eq!(
            duplex.tick(1_400, &floor, false),
            vec![Gate(GateCmd::Pause)]
        );
        assert_eq!(duplex.next_wakeup(), Some(1_900));
        assert_eq!(duplex.tick(1_899, &floor, false), vec![]);
        // Row 10: 900 ms reached, still voiced.
        assert_eq!(duplex.tick(1_900, &floor, false), interrupt());
        assert_eq!(duplex.next_wakeup(), None);
    }

    #[test]
    fn at_the_verdict_an_unvoiced_onset_with_no_cue_is_noise() {
        let mut duplex = streaming();
        barge_with_words(&mut duplex, "um the", false);
        let floor = agent_speaking(1_500);
        let offset = DuplexEvent::Offset {
            at: 1_300,
            voiced_ms: 300,
        };
        assert_eq!(duplex.on_event(1_300, offset, &floor, false), vec![]);
        assert_eq!(
            duplex.tick(1_400, &floor, false),
            vec![Gate(GateCmd::Pause)]
        );
        // Words over the agent: it stays paused while the front reads them.
        assert_eq!(duplex.tick(1_900, &floor, false), vec![]);
        assert_eq!(
            duplex.tick(3_400, &floor, false),
            vec![classified(Class::Noise), Gate(GateCmd::Resume)]
        );
    }

    #[test]
    fn row_1_the_agents_own_words_are_echo() {
        let mut duplex = streaming();
        assert_eq!(
            barge_with_words(&mut duplex, "two flights under four hundred", false),
            vec![classified(Class::Echo), Gate(GateCmd::Resume)]
        );
    }

    #[test]
    fn the_front_decides_a_stop_in_context() {
        // No word decides anything: the front is asked, the agent stays ducked meanwhile.
        for cue in [
            "stop talking",
            "top talking",
            "hold on",
            "yes",
            "stop by the store",
        ] {
            let mut duplex = streaming();
            assert_eq!(
                barge_with_words(&mut duplex, cue, false),
                vec![asked(1, cue)],
                "{cue}"
            );
            // A stop, a near miss of one, a request: the front's act takes the floor.
            assert_eq!(
                verdict(&mut duplex, 1_300, Class::Interrupt),
                interrupt(),
                "{cue}"
            );
        }
        // A newer interim asks again; the same words are not asked twice.
        let mut duplex = streaming();
        barge_with_words(&mut duplex, "stop", false);
        let floor = agent_speaking(1_250);
        assert_eq!(
            duplex.on_stt(1_250, "stop", false, false, &floor, false),
            vec![]
        );
        assert_eq!(
            duplex.on_stt(1_300, "stop talking", false, false, &floor, false),
            vec![asked(1, "stop talking")]
        );
        // A verdict for another turn changes nothing.
        let other = duplex.front_verdict(1_350, TurnId(9), Class::Interrupt, &floor);
        assert_eq!(other, vec![]);
    }

    #[test]
    fn row_5_continuers_resume_from_the_duck_once_the_caller_stops() {
        let mut duplex = streaming();
        assert_eq!(
            barge_with_words(&mut duplex, "uh huh, right", false),
            vec![asked(1, "uh huh right")]
        );
        // The front's continuer lets the agent go on.
        assert_eq!(
            verdict(&mut duplex, 1_300, Class::Backchannel),
            vec![classified(Class::Backchannel), Gate(GateCmd::Resume)]
        );
        assert_eq!(duplex.next_wakeup(), None);
    }

    #[test]
    fn row_6_side_talk_pauses_and_resumes_after_silence() {
        let mut duplex = streaming();
        assert_eq!(
            barge_with_words(&mut duplex, "not you, honey", false),
            vec![asked(1, "not you honey")]
        );
        assert_eq!(
            verdict(&mut duplex, 1_200, Class::TalkingToOthers),
            vec![classified(Class::TalkingToOthers), Gate(GateCmd::Pause)]
        );
        let floor = agent_speaking(2_000);
        duplex.on_event(
            1_500,
            DuplexEvent::Offset {
                at: 1_500,
                voiced_ms: 500,
            },
            &floor,
            false,
        );
        let silence = DuplexEvent::Silence { at: 1_700, ms: 200 };
        assert_eq!(duplex.on_event(1_700, silence, &floor, false), vec![]);
        let silence = DuplexEvent::Silence { at: 2_200, ms: 700 };
        assert_eq!(
            duplex.on_event(2_200, silence, &floor, false),
            vec![Gate(GateCmd::Resume)]
        );
    }

    #[test]
    fn side_talk_is_cut_at_the_maximum_pause_and_at_six_seconds_at_the_latest() {
        let mut duplex = streaming();
        barge_with_words(&mut duplex, "not you, honey", false);
        verdict(&mut duplex, 1_200, Class::TalkingToOthers);
        // Paused at 1,200 ms: the maximum pause of 2,500 ms ends at 3,700 ms.
        assert_eq!(duplex.next_wakeup(), Some(3_700));
        let floor = agent_speaking(3_700);
        assert_eq!(duplex.tick(3_699, &floor, false), vec![]);
        assert_eq!(
            duplex.tick(3_700, &floor, false),
            vec![Cut(CutCause::SideTalkTimeout), UserSpeaking(true)]
        );

        let cfg = DuplexConfig {
            max_pause_ms: 10_000,
            ..DuplexConfig::default()
        };
        let mut duplex = Duplex::new(cfg, lexicon(), false);
        barge_with_words(&mut duplex, "not you, honey", false);
        verdict(&mut duplex, 1_200, Class::TalkingToOthers);
        assert_eq!(duplex.next_wakeup(), Some(7_000));
    }

    #[test]
    fn row_7_a_voice_far_below_the_callers_level_is_background() {
        let mut duplex = streaming();
        // One ordinary turn sets the caller's level (-30 dB).
        duplex.on_event(0, onset(0), &quiet(), false);
        sure(&mut duplex, 450);
        duplex.on_stt(500, "book a table", true, true, &quiet(), false);
        let floor = agent_speaking(1_000);
        let far = |at| DuplexEvent::Onset {
            at,
            level_db: -43.0,
            reflex_ducked: true,
        };
        // The 64 ms of an onset are not yet a verdict.
        assert_eq!(duplex.on_event(1_000, far(1_000), &floor, false), vec![]);
        let level = DuplexEvent::Level { level_db: -42.5 };
        assert_eq!(duplex.on_event(1_300, level, &floor, false), vec![]);
        // Still that far below where the agent would pause: it goes on instead.
        assert_eq!(
            duplex.tick(1_400, &floor, false),
            vec![classified(Class::Background), Gate(GateCmd::Resume)]
        );
        // A shorter sound is judged when it ends.
        let floor = agent_speaking(5_000);
        duplex.on_event(5_000, far(5_000), &floor, false);
        let offset = DuplexEvent::Offset {
            at: 5_300,
            voiced_ms: 300,
        };
        assert_eq!(
            duplex.on_event(5_300, offset, &floor, false),
            vec![classified(Class::Background), Gate(GateCmd::Resume)]
        );
        // Within 12 dB it is the caller.
        let near = DuplexEvent::Onset {
            at: 9_000,
            level_db: -40.0,
            reflex_ducked: true,
        };
        let floor = agent_speaking(9_000);
        assert_eq!(duplex.on_event(9_000, near, &floor, false), vec![]);
        assert_eq!(
            duplex.tick(9_400, &floor, false),
            vec![Gate(GateCmd::Pause)]
        );
    }

    #[test]
    fn row_7_an_interruption_that_starts_softly_is_still_the_caller() {
        let mut duplex = uploading();
        duplex.on_event(0, onset(0), &quiet(), false);
        duplex.on_stt(500, "book a table", true, true, &quiet(), false);
        // The first 64 ms are 15 dB below the caller's level, the word itself is not.
        let floor = agent_speaking(1_000);
        let soft = DuplexEvent::Onset {
            at: 1_000,
            level_db: -45.0,
            reflex_ducked: true,
        };
        assert_eq!(duplex.on_event(1_000, soft, &floor, false), vec![]);
        let level = DuplexEvent::Level { level_db: -33.0 };
        assert_eq!(duplex.on_event(1_130, level, &floor, false), vec![]);
        assert_eq!(
            duplex.tick(1_400, &floor, false),
            vec![Gate(GateCmd::Pause)]
        );
        assert_eq!(duplex.tick(1_900, &floor, false), interrupt());
    }

    #[test]
    fn row_8_a_short_sound_with_no_words_is_noise() {
        let mut duplex = uploading();
        let floor = agent_speaking(1_000);
        duplex.on_event(1_000, onset(1_000), &floor, false);
        let offset = DuplexEvent::Offset {
            at: 1_150,
            voiced_ms: 200,
        };
        assert_eq!(
            duplex.on_event(1_150, offset, &floor, false),
            vec![classified(Class::Noise), Gate(GateCmd::Resume)]
        );
    }

    #[test]
    fn row_9_a_longer_sound_that_ends_before_the_verdict_is_a_backchannel() {
        let mut duplex = uploading();
        let floor = agent_speaking(1_000);
        duplex.on_event(1_000, onset(1_000), &floor, false);
        assert_eq!(
            duplex.tick(1_400, &floor, false),
            vec![Gate(GateCmd::Pause)]
        );
        let offset = DuplexEvent::Offset {
            at: 1_500,
            voiced_ms: 500,
        };
        assert_eq!(
            duplex.on_event(1_500, offset, &floor, false),
            vec![classified(Class::Backchannel), Gate(GateCmd::Resume)]
        );
    }

    #[test]
    fn a_stop_cue_whose_words_come_after_the_verdict_still_cuts() {
        // A short "stop": its sound is over, and judged a backchannel, before any transcript.
        let short_sound = |duplex: &mut Duplex| {
            let floor = agent_speaking(1_000);
            duplex.on_event(1_000, onset(1_000), &floor, false);
            let offset = DuplexEvent::Offset {
                at: 1_520,
                voiced_ms: 300,
            };
            assert_eq!(
                duplex.on_event(1_520, offset, &floor, false),
                vec![classified(Class::Backchannel), Gate(GateCmd::Resume)]
            );
        };
        let mut duplex = streaming();
        short_sound(&mut duplex);
        let floor = agent_speaking(1_700);
        assert_eq!(
            duplex.on_stt(1_700, "stop", false, false, &floor, false),
            vec![asked(1, "stop")]
        );
        assert_eq!(verdict(&mut duplex, 1_750, Class::Interrupt), interrupt());
        // Once: the final repeats the words, not the cut.
        sure(&mut duplex, 1_900);
        let actions = duplex.on_stt(2_000, "Stop.", true, true, &quiet(), false);
        assert_eq!(actions.len(), 2, "{actions:?}");
        assert_eq!(end_of_turn(&actions).class, Class::Interrupt);

        // Request-response speech-to-text has the words only with the final: the front
        // decides them as the turn (a control, applied by the actor), nothing cuts before.
        let mut duplex = uploading();
        short_sound(&mut duplex);
        let actions = duplex.on_stt(2_300, "Wait a second.", true, true, &floor, false);
        assert!(!actions.contains(&Cut(CutCause::Interrupt)), "{actions:?}");
        assert_eq!(end_of_turn(&actions).text, "Wait a second.");

        // A real backchannel stays one, and so does a cue once the agent is silent.
        let mut duplex = streaming();
        short_sound(&mut duplex);
        assert_eq!(
            duplex.on_stt(1_700, "uh huh", false, false, &floor, false),
            vec![asked(1, "uh huh")]
        );
        let mut duplex = streaming();
        short_sound(&mut duplex);
        assert_eq!(
            duplex.on_stt(1_700, "stop", false, false, &quiet(), false),
            vec![]
        );
    }

    #[test]
    fn row_10_still_voiced_at_the_verdict_interrupts_without_words() {
        let mut duplex = uploading();
        let floor = agent_speaking(1_000);
        duplex.on_event(1_000, onset(1_000), &floor, false);
        duplex.tick(1_400, &floor, false);
        assert_eq!(duplex.tick(1_900, &floor, false), interrupt());
    }

    #[test]
    fn an_onset_with_the_agent_silent_gives_the_caller_the_floor() {
        let mut duplex = streaming();
        assert_eq!(
            duplex.on_event(0, onset_alone(0), &quiet(), false),
            vec![UserSpeaking(true)]
        );
        assert_eq!(duplex.next_wakeup(), None);
        // Barge-in switched off: the same over agent audio (the analyser does not duck).
        let cfg = DuplexConfig {
            barge_in: false,
            ..DuplexConfig::default()
        };
        let mut duplex = Duplex::new(cfg, lexicon(), false);
        assert_eq!(
            duplex.on_event(0, onset_alone(0), &agent_speaking(0), false),
            vec![UserSpeaking(true)]
        );
    }

    #[test]
    fn an_analyser_that_stops_does_not_leave_the_caller_speaking() {
        let mut duplex = uploading();
        let floor = quiet();
        assert_eq!(
            duplex.on_event(0, onset_alone(0), &floor, false),
            vec![UserSpeaking(true)]
        );
        let stopped = DuplexEvent::Degraded("interaction_missing_build_features");
        assert_eq!(
            duplex.on_event(500, stopped, &floor, false),
            vec![UserSpeaking(false)]
        );
        assert_eq!(duplex.next_wakeup(), None);
    }

    #[test]
    fn without_an_analyser_the_first_words_are_the_onset() {
        let mut duplex = streaming();
        let floor = agent_speaking(1_000);
        assert_eq!(
            duplex.on_stt(1_000, "is it", false, false, &floor, false),
            vec![Gate(GateCmd::Duck), asked(1, "is it")]
        );
        assert_eq!(
            duplex.on_stt(1_100, "stop", false, false, &floor, false),
            vec![asked(1, "stop")]
        );
        assert_eq!(verdict(&mut duplex, 1_150, Class::Interrupt), interrupt());
        let actions = duplex.on_stt(1_500, "stop", true, true, &quiet(), false);
        let turn = end_of_turn(&actions);
        assert_eq!(
            (turn.onset_at, turn.attribution),
            (None, Attribution::Medium)
        );
        assert_eq!(turn.class, Class::Interrupt);
    }

    #[test]
    fn request_response_uploads_voiced_audio_when_the_turn_ends() {
        // Smart Turn sure at 200 ms of silence.
        let mut duplex = uploading();
        let floor = quiet();
        duplex.on_event(0, onset(0), &floor, false);
        duplex.on_event(
            900,
            DuplexEvent::Offset {
                at: 900,
                voiced_ms: 900,
            },
            &floor,
            false,
        );
        let silence = DuplexEvent::Silence { at: 1_100, ms: 200 };
        assert_eq!(duplex.on_event(1_100, silence, &floor, false), vec![]);
        let sure = DuplexEvent::SmartTurn { at: 1_120, p: 0.75 };
        assert_eq!(
            duplex.on_event(1_120, sure, &floor, false),
            vec![DuplexAction::FinalizeStt { upload: true }]
        );
        assert_eq!(duplex.next_wakeup(), Some(4_120));
        // The later silence reports do not flush again.
        let silence = DuplexEvent::Silence {
            at: 2_400,
            ms: 1_500,
        };
        assert_eq!(duplex.on_event(2_400, silence, &floor, false), vec![]);
        let actions = duplex.on_stt(2_500, "Book a table for two.", true, true, &floor, false);
        assert_eq!(actions[0], UserSpeaking(false));
        let turn = end_of_turn(&actions);
        assert_eq!(turn.text, "Book a table for two.");
        assert_eq!(
            (turn.at, turn.onset_at, turn.acoustic_end),
            (2_500, Some(0), Some(900))
        );
        assert_eq!(turn.source, EotSource::SmartTurn);
        assert_eq!(turn.attribution, Attribution::High);
        assert_eq!(duplex.next_wakeup(), None);

        // Smart Turn unsure: the 1,500 ms ceiling.
        let mut duplex = uploading();
        duplex.on_event(0, onset(0), &floor, false);
        duplex.on_event(
            900,
            DuplexEvent::Offset {
                at: 900,
                voiced_ms: 900,
            },
            &floor,
            false,
        );
        duplex.on_event(
            1_100,
            DuplexEvent::Silence { at: 1_100, ms: 200 },
            &floor,
            false,
        );
        let unsure = DuplexEvent::SmartTurn { at: 1_120, p: 0.3 };
        // A pause in a turn that goes on: the moment for a listening sound.
        let listen = DuplexAction::Backchannel {
            text: String::new(),
            voiced_ms: 900,
        };
        assert_eq!(duplex.on_event(1_120, unsure, &floor, false), vec![listen]);
        let silence = DuplexEvent::Silence { at: 1_600, ms: 700 };
        assert_eq!(duplex.on_event(1_600, silence, &floor, false), vec![]);
        let silence = DuplexEvent::Silence {
            at: 2_400,
            ms: 1_500,
        };
        assert_eq!(
            duplex.on_event(2_400, silence, &floor, false),
            vec![DuplexAction::FinalizeStt { upload: true }]
        );
        let actions = duplex.on_stt(2_900, "book a table", true, true, &floor, false);
        assert_eq!(end_of_turn(&actions).source, EotSource::SilenceCeiling);
    }

    #[test]
    fn an_answer_to_a_question_ends_at_its_first_judged_pause() {
        // Live MA2: "Window, please." had Smart Turn at 0.49 and waited for the ceiling.
        let floor = quiet();
        let pause = |duplex: &mut Duplex, p: f32, expected: bool| {
            duplex.on_event(0, onset(0), &floor, expected);
            let offset = DuplexEvent::Offset {
                at: 800,
                voiced_ms: 800,
            };
            duplex.on_event(800, offset, &floor, expected);
            let silence = DuplexEvent::Silence { at: 1_000, ms: 200 };
            duplex.on_event(1_000, silence, &floor, expected);
            let verdict = DuplexEvent::SmartTurn { at: 1_020, p };
            duplex.on_event(1_020, verdict, &floor, expected)
        };
        let upload = vec![DuplexAction::FinalizeStt { upload: true }];
        assert_eq!(pause(&mut uploading(), 0.49, true), upload);
        // Not when no answer is expected (a listening sound instead), nor when Smart Turn is
        // sure the caller has not finished.
        assert!(!pause(&mut uploading(), 0.49, false).contains(&upload[0]));
        assert_eq!(pause(&mut uploading(), 0.02, true), vec![]);
    }

    #[test]
    fn request_response_keeps_the_turn_open_when_the_caller_goes_on_during_the_upload() {
        let mut duplex = uploading();
        let floor = quiet();
        let offset = |at, voiced_ms| DuplexEvent::Offset { at, voiced_ms };
        let silence = |at| DuplexEvent::Silence { at, ms: 200 };
        let sure = |at| DuplexEvent::SmartTurn { at, p: 0.9 };
        let upload = vec![DuplexAction::FinalizeStt { upload: true }];

        // A pause Smart Turn takes for the end, and the caller goes on before the final.
        duplex.on_event(0, onset(0), &floor, false);
        duplex.on_event(900, offset(900, 900), &floor, false);
        duplex.on_event(1_100, silence(1_100), &floor, false);
        assert_eq!(duplex.on_event(1_120, sure(1_120), &floor, false), upload);
        assert_eq!(
            duplex.on_event(1_300, onset_alone(1_300), &floor, false),
            vec![]
        );
        let first = duplex.on_stt(
            1_500,
            "I need a flight to Denver.",
            true,
            true,
            &floor,
            false,
        );
        assert_eq!(first, vec![]);

        // The rest is uploaded on its own; E carries both parts, and the caller held the
        // floor all along.
        duplex.on_event(2_300, offset(2_300, 1_000), &floor, false);
        duplex.on_event(2_500, silence(2_500), &floor, false);
        assert_eq!(duplex.on_event(2_520, sure(2_520), &floor, false), upload);
        let actions = duplex.on_stt(2_800, "Ideally under 400.", true, true, &floor, false);
        assert_eq!(actions[0], UserSpeaking(false));
        let turn = end_of_turn(&actions);
        assert_eq!(turn.text, "I need a flight to Denver. Ideally under 400.");
        assert_eq!((turn.turn, turn.onset_at), (TurnId(1), Some(0)));

        // The rest was over before the first transcript came back: it is uploaded at once.
        let mut duplex = uploading();
        duplex.on_event(0, onset(0), &floor, false);
        duplex.on_event(900, offset(900, 900), &floor, false);
        duplex.on_event(1_100, silence(1_100), &floor, false);
        duplex.on_event(1_120, sure(1_120), &floor, false);
        duplex.on_event(1_300, onset(1_300), &floor, false);
        duplex.on_event(1_900, offset(1_900, 400), &floor, false);
        duplex.on_event(2_100, silence(2_100), &floor, false);
        assert_eq!(duplex.on_event(2_120, sure(2_120), &floor, false), vec![]);
        let first = duplex.on_stt(2_200, "Book a table.", true, true, &floor, false);
        assert_eq!(first, upload);
        // That upload fails: the turn ends with the words it has.
        let actions = duplex.on_stt_error(2_500);
        let turn = end_of_turn(&actions);
        assert_eq!(
            (turn.text.as_str(), turn.source),
            ("Book a table.", EotSource::SmartTurn)
        );

        // Only a noise followed: the held words are the turn at the silence ceiling.
        let mut duplex = uploading();
        duplex.on_event(0, onset(0), &floor, false);
        duplex.on_event(900, offset(900, 900), &floor, false);
        duplex.on_event(1_100, silence(1_100), &floor, false);
        duplex.on_event(1_120, sure(1_120), &floor, false);
        duplex.on_event(1_300, onset(1_300), &floor, false);
        duplex.on_stt(1_500, "Book a table.", true, true, &floor, false);
        duplex.on_event(1_400, offset(1_400, 100), &floor, false);
        let ceiling = DuplexEvent::Silence {
            at: 2_900,
            ms: 1_500,
        };
        let actions = duplex.on_event(2_900, ceiling, &floor, false);
        assert_eq!(actions[0], DuplexAction::FinalizeStt { upload: false });
        assert_eq!(end_of_turn(&actions).text, "Book a table.");
    }

    #[test]
    fn request_response_never_uploads_under_250_ms_of_voice() {
        let mut duplex = uploading();
        let floor = quiet();
        assert_eq!(
            duplex.on_event(0, onset_alone(0), &floor, false),
            vec![UserSpeaking(true)]
        );
        duplex.on_event(
            200,
            DuplexEvent::Offset {
                at: 200,
                voiced_ms: 200,
            },
            &floor,
            false,
        );
        duplex.on_event(
            400,
            DuplexEvent::Silence { at: 400, ms: 200 },
            &floor,
            false,
        );
        let sure = DuplexEvent::SmartTurn { at: 420, p: 0.9 };
        assert_eq!(duplex.on_event(420, sure, &floor, false), vec![]);
        let silence = DuplexEvent::Silence {
            at: 1_700,
            ms: 1_500,
        };
        assert_eq!(
            duplex.on_event(1_700, silence, &floor, false),
            vec![
                DuplexAction::FinalizeStt { upload: false },
                UserSpeaking(false)
            ]
        );
        // The buffer is dropped once, and no final is awaited.
        assert_eq!(duplex.tick(1_800, &floor, false), vec![]);
        assert_eq!(duplex.next_wakeup(), None);
    }

    #[test]
    fn no_final_within_three_seconds_of_an_upload_ends_the_turn_empty() {
        let mut duplex = uploading();
        let floor = quiet();
        duplex.on_event(0, onset(0), &floor, false);
        duplex.on_event(
            900,
            DuplexEvent::Offset {
                at: 900,
                voiced_ms: 900,
            },
            &floor,
            false,
        );
        duplex.on_event(
            2_400,
            DuplexEvent::Silence {
                at: 2_400,
                ms: 1_500,
            },
            &floor,
            false,
        );
        assert_eq!(duplex.tick(5_399, &floor, false), vec![]);
        let actions = duplex.tick(5_400, &floor, false);
        assert_eq!(actions[0], UserSpeaking(false));
        let turn = end_of_turn(&actions);
        assert_eq!(
            (turn.text.as_str(), turn.source),
            ("", EotSource::SttFailed)
        );
        assert!(!turn.suspected_echo);
        assert_eq!(duplex.next_wakeup(), None);

        // An error of the speech-to-text ends the turn the same way, at once.
        let mut duplex = uploading();
        assert_eq!(duplex.on_stt_error(10), vec![]);
        duplex.on_event(100, onset(100), &floor, false);
        let actions = duplex.on_stt_error(600);
        let turn = end_of_turn(&actions);
        assert_eq!(
            (turn.text.as_str(), turn.source),
            ("", EotSource::SttFailed)
        );
    }

    #[test]
    fn a_final_made_of_the_agents_words_is_suspected_echo() {
        let said = |ended: Ms| FloorSnapshot {
            recent_agent_lines: vec![(ended, AGENT_LINE.to_owned())],
            ..quiet()
        };
        let mut duplex = uploading();
        duplex.on_event(10_000, onset(10_000), &quiet(), false);
        // The line ended 4 s before the onset: inside the window.
        let actions = duplex.on_stt(
            12_000,
            "two flights under four hundred",
            true,
            true,
            &said(6_000),
            false,
        );
        let turn = end_of_turn(&actions);
        assert!(turn.suspected_echo);
        assert_eq!(
            (turn.attribution, turn.class),
            (Attribution::Low, Class::Echo)
        );

        // The same words 6 s after the line ended are the caller's.
        duplex.on_event(20_000, onset(20_000), &quiet(), false);
        let actions = duplex.on_stt(
            22_000,
            "two flights under four hundred",
            true,
            true,
            &said(14_000),
            false,
        );
        assert!(!end_of_turn(&actions).suspected_echo);

        // Fewer than 60% of the words, or fewer than 3 words, is not echo.
        duplex.on_event(30_000, onset(30_000), &quiet(), false);
        let actions = duplex.on_stt(
            32_000,
            "book two seats for tomorrow",
            true,
            true,
            &said(31_000),
            false,
        );
        assert!(!end_of_turn(&actions).suspected_echo);
        duplex.on_event(40_000, onset(40_000), &quiet(), false);
        let actions = duplex.on_stt(42_000, "two flights", true, true, &said(41_000), false);
        assert!(!end_of_turn(&actions).suspected_echo);
    }

    /// Words, an offset, 200 ms of silence and a sure Smart Turn: the eager end of turn.
    fn eager(duplex: &mut Duplex, text: &str) -> Vec<DuplexAction> {
        let floor = quiet();
        duplex.on_event(0, onset(0), &floor, false);
        duplex.on_stt(800, text, false, false, &floor, false);
        duplex.on_event(
            900,
            DuplexEvent::Offset {
                at: 900,
                voiced_ms: 900,
            },
            &floor,
            false,
        );
        duplex.on_event(
            1_100,
            DuplexEvent::Silence { at: 1_100, ms: 200 },
            &floor,
            false,
        );
        duplex.on_event(
            1_120,
            DuplexEvent::SmartTurn { at: 1_120, p: 0.85 },
            &floor,
            false,
        )
    }

    #[test]
    fn eager_end_of_turn_with_the_same_and_with_different_final_words() {
        let mut duplex = streaming();
        let text = "book a table for two".to_owned();
        assert_eq!(
            eager(&mut duplex, &text),
            vec![DuplexAction::Eager {
                turn: TurnId(1),
                text
            }]
        );
        let actions = duplex.on_stt(1_400, "Book a table for two.", true, true, &quiet(), false);
        let turn = end_of_turn(&actions);
        assert_eq!((turn.turn, turn.eager_same), (TurnId(1), Some(true)));

        let mut duplex = streaming();
        eager(&mut duplex, "book a table for two");
        let actions = duplex.on_stt(1_400, "Book a table for ten.", true, true, &quiet(), false);
        assert_eq!(end_of_turn(&actions).eager_same, Some(false));

        // Under 0.8, or switched off, there is none.
        let mut duplex = streaming();
        let floor = quiet();
        duplex.on_stt(800, "book a table", false, false, &floor, false);
        let unsure = DuplexEvent::SmartTurn { at: 1_120, p: 0.75 };
        assert_eq!(duplex.on_event(1_120, unsure, &floor, false), vec![]);
        let cfg = DuplexConfig {
            eager: false,
            ..DuplexConfig::default()
        };
        let mut duplex = Duplex::new(cfg, lexicon(), false);
        assert_eq!(eager(&mut duplex, "book a table"), vec![]);
    }

    /// Deepgram Flux: its `EagerEndOfTurn` is the eager signal and its `TurnResumed`
    /// withdraws it; Smart Turn makes no guess of its own, and `EndOfTurn` (the speech-final)
    /// is E, compared with the guess.
    #[test]
    fn a_provider_turn_model_gives_the_eager_end_of_turn() {
        let flux = || streaming().with_provider_turns(true);
        let floor = quiet();
        // A sure Smart Turn guesses nothing.
        let mut duplex = flux();
        assert_eq!(eager(&mut duplex, "book a table for two"), vec![]);
        let guess = duplex.on_stt(1_200, "book a table for two", false, false, &floor, false);
        assert_eq!(guess, vec![]);
        assert_eq!(
            duplex.on_turn_event(SttTurnEvent::EagerEndOfTurn, &floor),
            vec![DuplexAction::Eager {
                turn: TurnId(1),
                text: "book a table for two".to_owned()
            }]
        );
        let actions = duplex.on_stt(1_400, "Book a table for two.", true, true, &floor, false);
        assert_eq!(end_of_turn(&actions).eager_same, Some(true));
        // Resumed: withdrawn, and a second guess is still allowed.
        let mut duplex = flux();
        duplex.on_stt(800, "book a table", false, false, &floor, false);
        assert_eq!(
            duplex
                .on_turn_event(SttTurnEvent::EagerEndOfTurn, &floor)
                .len(),
            1
        );
        assert_eq!(
            duplex.on_turn_event(SttTurnEvent::TurnResumed, &floor),
            vec![DuplexAction::EagerWithdrawn { turn: TurnId(1) }]
        );
        assert_eq!(
            duplex.on_turn_event(SttTurnEvent::TurnResumed, &floor),
            vec![]
        );
        duplex.on_stt(1_600, "book a table for two", false, false, &floor, false);
        assert_eq!(
            duplex.on_turn_event(SttTurnEvent::EagerEndOfTurn, &floor),
            vec![DuplexAction::Eager {
                turn: TurnId(1),
                text: "book a table for two".to_owned()
            }]
        );
        // Eager mode off: the provider's guess is not taken; no turn: nothing to guess.
        let cfg = DuplexConfig {
            eager: false,
            ..DuplexConfig::default()
        };
        let mut duplex = Duplex::new(cfg, lexicon(), false).with_provider_turns(true);
        duplex.on_stt(800, "book a table", false, false, &floor, false);
        assert_eq!(
            duplex.on_turn_event(SttTurnEvent::EagerEndOfTurn, &floor),
            vec![]
        );
        assert_eq!(
            flux().on_turn_event(SttTurnEvent::EagerEndOfTurn, &floor),
            vec![]
        );
    }

    #[test]
    fn an_eager_end_of_turn_is_withdrawn_when_the_caller_goes_on() {
        // New words.
        let mut duplex = streaming();
        let floor = quiet();
        eager(&mut duplex, "book a table");
        // Still sure with the new words: the second and last eager of the turn.
        assert_eq!(
            duplex.on_stt(1_300, "book a table for", false, false, &floor, false),
            vec![
                DuplexAction::EagerWithdrawn { turn: TurnId(1) },
                DuplexAction::Eager {
                    turn: TurnId(1),
                    text: "book a table for".to_owned()
                }
            ]
        );
        assert_eq!(
            duplex.on_stt(1_400, "book a table for two", false, false, &floor, false),
            vec![DuplexAction::EagerWithdrawn { turn: TurnId(1) }]
        );
        let actions = duplex.on_stt(1_900, "book a table for two", true, true, &floor, false);
        assert_eq!(end_of_turn(&actions).eager_same, None);

        // 300 ms more of voice.
        let mut duplex = streaming();
        eager(&mut duplex, "book a table");
        assert_eq!(
            duplex.on_event(1_500, onset_alone(1_500), &floor, false),
            vec![]
        );
        assert_eq!(duplex.next_wakeup(), Some(1_800));
        assert_eq!(duplex.tick(1_799, &floor, false), vec![]);
        assert_eq!(
            duplex.tick(1_800, &floor, false),
            vec![DuplexAction::EagerWithdrawn { turn: TurnId(1) }]
        );
    }

    #[test]
    fn a_sure_smart_turn_with_a_buffered_final_claims_the_segment() {
        let mut duplex = streaming();
        let floor = quiet();
        duplex.on_event(0, onset(0), &floor, false);
        duplex.on_stt(800, "book a table", true, false, &floor, false);
        duplex.on_event(
            900,
            DuplexEvent::Offset {
                at: 900,
                voiced_ms: 900,
            },
            &floor,
            false,
        );
        duplex.on_event(
            1_100,
            DuplexEvent::Silence { at: 1_100, ms: 200 },
            &floor,
            false,
        );
        let sure = DuplexEvent::SmartTurn { at: 1_120, p: 0.9 };
        // One claim, and no eager start: E is on its way.
        assert_eq!(
            duplex.on_event(1_120, sure, &floor, false),
            vec![DuplexAction::ClaimSegment]
        );
        assert_eq!(duplex.tick(1_200, &floor, false), vec![]);
        let actions = duplex.on_stt(1_150, "book a table", true, true, &floor, false);
        assert_eq!(end_of_turn(&actions).source, EotSource::SmartTurn);

        // Only an interim: the VoiceManager's own timers end the turn.
        let mut duplex = streaming();
        duplex.on_event(0, onset(0), &floor, false);
        duplex.on_stt(800, "book a table", false, false, &floor, false);
        duplex.on_event(
            900,
            DuplexEvent::Offset {
                at: 900,
                voiced_ms: 900,
            },
            &floor,
            false,
        );
        duplex.on_event(
            1_100,
            DuplexEvent::Silence { at: 1_100, ms: 200 },
            &floor,
            false,
        );
        let sure = DuplexEvent::SmartTurn { at: 1_120, p: 0.75 };
        assert_eq!(duplex.on_event(1_120, sure, &floor, false), vec![]);
    }

    /// Live TP1: after a held thinking pause, an unsure verdict at the next pause holds it too.
    #[test]
    fn after_a_held_pause_an_unsure_verdict_holds_the_next_one() {
        let mut duplex = streaming();
        let floor = quiet();
        let offset = |at| DuplexEvent::Offset { at, voiced_ms: 900 };
        let judged = |at, p| DuplexEvent::SmartTurn { at, p };
        duplex.on_event(0, onset(0), &floor, false);
        duplex.on_event(900, offset(900), &floor, false);
        duplex.on_event(1_100, judged(1_100, 0.04), &floor, false);
        let first = duplex.on_stt(
            1_400,
            "we want somewhere warm by the sea",
            true,
            true,
            &floor,
            false,
        );
        assert!(first.is_empty(), "{first:?}");
        duplex.on_event(2_100, onset(2_100), &floor, false);
        duplex.on_event(3_000, offset(3_000), &floor, false);
        duplex.on_event(3_200, judged(3_200, 0.57), &floor, false);
        let second = duplex.on_stt(3_500, "where the kids can swim", true, true, &floor, false);
        assert!(
            !second
                .iter()
                .any(|action| matches!(action, DuplexAction::EndOfTurn(_))),
            "{second:?}"
        );
        duplex.on_event(4_200, onset(4_200), &floor, false);
        duplex.on_event(5_100, offset(5_100), &floor, false);
        duplex.on_event(5_300, judged(5_300, 0.97), &floor, false);
        let last = duplex.on_stt(5_600, "and a hotel by the beach", true, true, &floor, false);
        let eot = end_of_turn(&last);
        assert!(
            eot.text.contains("sea") && eot.text.contains("beach"),
            "{}",
            eot.text
        );

        // Without the earlier hold an unsure verdict on words that do not dangle ends it.
        let mut duplex = streaming();
        duplex.on_event(0, onset(0), &floor, false);
        duplex.on_event(900, offset(900), &floor, false);
        duplex.on_event(1_100, judged(1_100, 0.57), &floor, false);
        let once = duplex.on_stt(1_400, "where the kids can swim", true, true, &floor, false);
        end_of_turn(&once);
    }

    /// Live TP1: Smart Turn was sure at a pause in a long story, the segment was claimed, and
    /// the caller spoke again 300 ms before the claimed final came: the turn ended there and
    /// the rest of the story became a second task. The words are kept and the turn goes on.
    #[test]
    fn a_caller_who_goes_on_while_the_claim_is_away_keeps_the_turn() {
        let mut duplex = streaming();
        let floor = quiet();
        duplex.on_event(0, onset(0), &floor, false);
        duplex.on_stt(800, "we want somewhere warm", true, false, &floor, false);
        let offset = |at| DuplexEvent::Offset { at, voiced_ms: 900 };
        duplex.on_event(900, offset(900), &floor, false);
        let silence = DuplexEvent::Silence { at: 1_100, ms: 200 };
        duplex.on_event(1_100, silence, &floor, false);
        let sure = DuplexEvent::SmartTurn { at: 1_120, p: 0.9 };
        assert_eq!(
            duplex.on_event(1_120, sure, &floor, false),
            vec![DuplexAction::ClaimSegment]
        );
        duplex.on_event(1_500, onset(1_500), &floor, false);
        let claimed = duplex.on_stt(1_800, "we want somewhere warm", true, true, &floor, false);
        assert!(
            !claimed
                .iter()
                .any(|action| matches!(action, DuplexAction::EndOfTurn(_))),
            "{claimed:?}"
        );
        duplex.on_stt(
            2_600,
            "and a hotel by the beach",
            true,
            false,
            &floor,
            false,
        );
        duplex.on_event(2_700, offset(2_700), &floor, false);
        let silence = DuplexEvent::Silence { at: 2_900, ms: 200 };
        duplex.on_event(2_900, silence, &floor, false);
        let sure = DuplexEvent::SmartTurn { at: 2_920, p: 0.9 };
        assert_eq!(
            duplex.on_event(2_920, sure, &floor, false),
            vec![DuplexAction::ClaimSegment]
        );
        let actions = duplex.on_stt(3_000, "and a hotel by the beach", true, true, &floor, false);
        let eot = end_of_turn(&actions);
        assert!(
            eot.text.contains("warm") && eot.text.contains("beach"),
            "{}",
            eot.text
        );
    }

    #[test]
    fn a_dangling_end_word_holds_the_turn_for_800_ms_once_per_pause() {
        let mut duplex = streaming();
        let floor = quiet();
        duplex.on_event(0, onset(0), &floor, false);
        duplex.on_event(
            900,
            DuplexEvent::Offset {
                at: 900,
                voiced_ms: 900,
            },
            &floor,
            false,
        );
        duplex.on_event(
            1_120,
            DuplexEvent::SmartTurn { at: 1_120, p: 0.4 },
            &floor,
            false,
        );
        assert_eq!(
            duplex.on_stt(1_500, "book a table and", true, true, &floor, false),
            vec![]
        );
        assert_eq!(duplex.next_wakeup(), Some(2_300));
        assert_eq!(duplex.tick(2_299, &floor, false), vec![]);
        let actions = duplex.tick(2_300, &floor, false);
        assert_eq!(end_of_turn(&actions).text, "book a table and");

        // New speech merges into the same turn, and its next pause may be held again.
        let mut duplex = streaming();
        duplex.on_event(0, onset(0), &floor, false);
        duplex.on_event(
            1_120,
            DuplexEvent::SmartTurn { at: 1_120, p: 0.4 },
            &floor,
            false,
        );
        assert_eq!(
            duplex.on_stt(1_500, "book a table and", true, true, &floor, false),
            vec![]
        );
        assert_eq!(
            duplex.on_event(1_900, onset_alone(1_900), &floor, false),
            vec![]
        );
        assert_eq!(duplex.next_wakeup(), None);
        duplex.on_event(
            2_700,
            DuplexEvent::SmartTurn { at: 2_700, p: 0.4 },
            &floor,
            false,
        );
        assert_eq!(
            duplex.on_stt(2_800, "a taxi to", true, true, &floor, false),
            vec![]
        );
        let actions = duplex.tick(3_600, &floor, false);
        let turn = end_of_turn(&actions);
        assert_eq!(
            (turn.turn, turn.text.as_str()),
            (TurnId(1), "book a table and a taxi to")
        );

        // A sure Smart Turn holds a dangling word too: it hears sound, not words (live take
        // 24: sure at a gap in "... the download numbers in the terminal?", the turn ended at
        // "the"). The words that follow merge into the turn.
        let mut duplex = streaming();
        duplex.on_event(0, onset(0), &floor, false);
        duplex.on_event(
            1_120,
            DuplexEvent::SmartTurn { at: 1_120, p: 0.9 },
            &floor,
            false,
        );
        let held = duplex.on_stt(
            1_500,
            "can you get the download numbers in the",
            true,
            true,
            &floor,
            false,
        );
        assert_eq!(held, vec![]);
        assert_eq!(duplex.next_wakeup(), Some(2_300));
        assert_eq!(
            duplex.on_stt(1_900, "terminal", false, false, &floor, false),
            vec![]
        );
        let actions = duplex.on_stt(2_100, "terminal?", true, true, &floor, false);
        assert_eq!(
            end_of_turn(&actions).text,
            "can you get the download numbers in the terminal?"
        );
        // Without more words it ends when the hold is over.
        let mut duplex = streaming();
        duplex.on_event(0, onset(0), &floor, false);
        duplex.on_event(
            1_120,
            DuplexEvent::SmartTurn { at: 1_120, p: 0.9 },
            &floor,
            false,
        );
        assert_eq!(
            duplex.on_stt(1_500, "book a table and", true, true, &floor, false),
            vec![]
        );
        assert_eq!(
            end_of_turn(&duplex.tick(2_300, &floor, false)).text,
            "book a table and"
        );
    }

    /// Live take 24 (28 Oct): "Yes. Go ahead." to a confirmation came from the speech-to-text
    /// 2.6 s after the caller stopped. The turn of that sound had been closed at the silence
    /// ceiling with no words; the words opened a turn with no onset, attributed medium, and
    /// the yes did not bind. Words that come soon after are that sound's: its onset, high.
    #[test]
    fn late_words_keep_the_onset_of_the_sound_they_are() {
        let floor = quiet();
        let spoken = |duplex: &mut Duplex| {
            duplex.on_event(1_000, onset(1_000), &floor, true);
            duplex.on_event(
                1_800,
                DuplexEvent::Offset {
                    at: 1_800,
                    voiced_ms: 800,
                },
                &floor,
                true,
            );
            duplex.on_event(
                3_300,
                DuplexEvent::Silence {
                    at: 3_300,
                    ms: 1_500,
                },
                &floor,
                true,
            );
        };
        let mut duplex = streaming();
        spoken(&mut duplex);
        let actions = duplex.on_stt(4_400, "Yes. Go ahead.", true, true, &floor, true);
        let turn = end_of_turn(&actions);
        assert_eq!(
            (turn.attribution, turn.onset_at),
            (Attribution::High, Some(1_000))
        );
        // Too late, or after another sound began, the words are a turn of their own.
        let mut duplex = streaming();
        spoken(&mut duplex);
        let actions = duplex.on_stt(3_300 + LATE_WORDS_MS + 1, "Yes.", true, true, &floor, true);
        assert_eq!(end_of_turn(&actions).attribution, Attribution::Medium);
    }

    /// A caller who spoke until `voiced_ms` and paused: the onset, the offset and, 200 ms into
    /// the pause, Smart Turn's verdict `p` when there is one.
    fn spoke(duplex: &mut Duplex, voiced_ms: u32, p: Option<f32>) {
        let floor = quiet();
        duplex.on_event(0, onset_alone(0), &floor, false);
        let at = Ms::from(voiced_ms);
        duplex.on_event(at, DuplexEvent::Offset { at, voiced_ms }, &floor, false);
        if let Some(p) = p {
            let at = at + 200;
            duplex.on_event(at, DuplexEvent::SmartTurn { at, p }, &floor, false);
        }
    }

    #[test]
    fn a_final_in_a_thinking_pause_holds_the_turn_until_the_caller_goes_on() {
        let floor = quiet();
        // Smart Turn is sure the caller has not finished; no word dangles.
        let mut duplex = streaming();
        spoke(&mut duplex, 1_500, Some(0.02));
        let final_ =
            |duplex: &mut Duplex, at, text| duplex.on_stt(at, text, true, true, &floor, false);
        assert_eq!(final_(&mut duplex, 2_000, "a lot of people I know"), vec![]);
        assert_eq!(duplex.next_wakeup(), Some(2_800));
        // The caller goes on: one turn, with all the words.
        assert_eq!(
            duplex.on_event(2_300, onset_alone(2_300), &floor, false),
            vec![]
        );
        assert_eq!(duplex.next_wakeup(), None);
        duplex.on_stt(2_600, "watch the vlogs", false, false, &floor, false);
        let offset = DuplexEvent::Offset {
            at: 3_000,
            voiced_ms: 700,
        };
        duplex.on_event(3_000, offset, &floor, false);
        sure(&mut duplex, 3_200);
        let actions = final_(&mut duplex, 3_300, "watch the vlogs.");
        assert_eq!(
            end_of_turn(&actions).text,
            "a lot of people I know watch the vlogs."
        );

        // Nobody goes on: the turn ends 800 ms after the final.
        let mut duplex = streaming();
        spoke(&mut duplex, 1_500, Some(0.02));
        assert_eq!(final_(&mut duplex, 2_000, "a lot of people I know"), vec![]);
        assert_eq!(duplex.tick(2_799, &floor, false), vec![]);
        let actions = duplex.tick(2_800, &floor, false);
        assert_eq!(end_of_turn(&actions).text, "a lot of people I know");

        // Unsure but not that sure (most such pauses were the caller's end): E at the final,
        // unless the words dangle (chapter 5's lexical hold).
        let mut duplex = streaming();
        spoke(&mut duplex, 1_500, Some(0.1));
        end_of_turn(&final_(&mut duplex, 2_000, "a lot of people I know"));
        let mut duplex = streaming();
        spoke(&mut duplex, 1_500, Some(0.1));
        assert_eq!(
            final_(&mut duplex, 2_000, "a lot of people I know and"),
            vec![]
        );
        assert_eq!(duplex.next_wakeup(), Some(2_800));
    }

    #[test]
    fn a_turn_that_began_over_the_agent_is_not_held_for_thinking() {
        // The caller interrupts the agent, stops, and the final comes before Smart Turn's
        // verdict (or with one sure the caller is thinking): the answer is not held.
        for p in [None, Some(0.02)] {
            let mut duplex = streaming();
            barge_with_words(&mut duplex, "what about Friday", false);
            verdict(&mut duplex, 1_300, Class::Interrupt);
            let floor = quiet();
            let offset = DuplexEvent::Offset {
                at: 1_800,
                voiced_ms: 800,
            };
            duplex.on_event(1_800, offset, &floor, false);
            if let Some(p) = p {
                duplex.on_event(
                    2_000,
                    DuplexEvent::SmartTurn { at: 2_000, p },
                    &floor,
                    false,
                );
            }
            let actions = duplex.on_stt(2_050, "what about Friday", true, true, &floor, false);
            assert_eq!(end_of_turn(&actions).at, 2_050, "{p:?}");
        }
        // The same words begun in the quiet are held: the pause takeover stays fixed.
        let mut duplex = streaming();
        spoke(&mut duplex, 1_500, None);
        let floor = quiet();
        assert_eq!(
            duplex.on_stt(1_700, "what about Friday", true, true, &floor, false),
            vec![]
        );
    }

    #[test]
    fn a_final_before_smart_turns_verdict_waits_for_it() {
        let floor = quiet();
        let final_ =
            |duplex: &mut Duplex| duplex.on_stt(1_700, "book a table", true, true, &floor, false);
        // A sure verdict ends the turn when it comes.
        let mut duplex = streaming();
        spoke(&mut duplex, 1_500, None);
        assert_eq!(final_(&mut duplex), vec![]);
        let actions = sure(&mut duplex, 1_850);
        let turn = end_of_turn(&actions);
        assert_eq!((turn.at, turn.text.as_str()), (1_850, "book a table"));
        // One sure the caller is thinking keeps the hold.
        let mut duplex = streaming();
        spoke(&mut duplex, 1_500, None);
        assert_eq!(final_(&mut duplex), vec![]);
        let thinking = DuplexEvent::SmartTurn { at: 1_850, p: 0.02 };
        assert!(
            !duplex
                .on_event(1_850, thinking, &floor, false)
                .iter()
                .any(|a| matches!(a, DuplexAction::EndOfTurn(_)))
        );
        assert_eq!(end_of_turn(&duplex.tick(2_500, &floor, false)).at, 2_500);
        // A merely unsure one ends it when it comes, unless the words dangle.
        let unsure = DuplexEvent::SmartTurn { at: 1_850, p: 0.2 };
        let mut duplex = streaming();
        spoke(&mut duplex, 1_500, None);
        assert_eq!(final_(&mut duplex), vec![]);
        let actions = duplex.on_event(1_850, unsure, &floor, false);
        let turn = end_of_turn(&actions);
        assert_eq!((turn.at, turn.text.as_str()), (1_850, "book a table"));
        let mut duplex = streaming();
        spoke(&mut duplex, 1_500, None);
        let dangling = duplex.on_stt(1_700, "book a table and", true, true, &floor, false);
        assert_eq!(dangling, vec![]);
        assert!(
            !duplex
                .on_event(1_850, unsure, &floor, false)
                .iter()
                .any(|a| matches!(a, DuplexAction::EndOfTurn(_)))
        );
        assert_eq!(duplex.next_wakeup(), Some(2_500));

        // Not held: a sure verdict that came first, an answer that is expected, words the
        // analyser never heard, request-response speech-to-text (its final follows an upload
        // that the verdict or the silence ceiling asked for), a turn model of the vendor's.
        let mut duplex = streaming();
        spoke(&mut duplex, 1_500, Some(0.9));
        end_of_turn(&final_(&mut duplex));
        let mut duplex = streaming();
        spoke(&mut duplex, 1_500, None);
        end_of_turn(&duplex.on_stt(1_700, "yes", true, true, &floor, true));
        let mut duplex = streaming();
        end_of_turn(&final_(&mut duplex));
        let mut duplex = uploading();
        spoke(&mut duplex, 1_500, Some(0.1));
        end_of_turn(&final_(&mut duplex));
        // A speech-to-text with its own turn model (Flux) ended the turn by that model.
        let mut duplex = streaming().with_provider_turns(true);
        spoke(&mut duplex, 1_500, Some(0.1));
        end_of_turn(&final_(&mut duplex));
    }

    #[test]
    fn a_final_that_comes_once_the_caller_speaks_again_does_not_end_the_turn() {
        let floor = quiet();
        // Smart Turn was sure at the pause, but the caller went on before the final came.
        let mut duplex = streaming();
        spoke(&mut duplex, 1_500, Some(0.95));
        assert_eq!(
            duplex.on_event(1_900, onset_alone(1_900), &floor, false),
            vec![]
        );
        let first = duplex.on_stt(
            2_000,
            "my fondest memories are from childhood.",
            true,
            true,
            &floor,
            false,
        );
        assert!(
            !first
                .iter()
                .any(|a| matches!(a, DuplexAction::EndOfTurn(_))),
            "{first:?}"
        );
        duplex.on_stt(2_300, "like the summer camp", false, false, &floor, false);
        let offset = DuplexEvent::Offset {
            at: 3_200,
            voiced_ms: 1_300,
        };
        duplex.on_event(3_200, offset, &floor, false);
        sure(&mut duplex, 3_400);
        let actions = duplex.on_stt(3_500, "like the summer camp.", true, true, &floor, false);
        let turn = end_of_turn(&actions);
        assert_eq!(
            (turn.turn, turn.text.as_str()),
            (
                TurnId(1),
                "my fondest memories are from childhood. like the summer camp."
            )
        );

        // The next pause is judged on its own: a final in it is held too.
        let mut duplex = streaming();
        spoke(&mut duplex, 1_500, Some(0.02));
        duplex.on_event(1_900, onset_alone(1_900), &floor, false);
        duplex.on_stt(2_000, "it's like", true, true, &floor, false);
        duplex.on_stt(2_300, "the alcohol", false, false, &floor, false);
        let offset = DuplexEvent::Offset {
            at: 3_200,
            voiced_ms: 1_300,
        };
        duplex.on_event(3_200, offset, &floor, false);
        let thinking = DuplexEvent::SmartTurn { at: 3_400, p: 0.02 };
        duplex.on_event(3_400, thinking, &floor, false);
        let held = duplex.on_stt(
            3_500,
            "the alcohol or none of it.",
            true,
            true,
            &floor,
            false,
        );
        assert!(
            !held.iter().any(|a| matches!(a, DuplexAction::EndOfTurn(_))),
            "{held:?}"
        );
        assert_eq!(duplex.next_wakeup(), Some(4_300));
    }

    #[test]
    fn attribution_follows_the_onset_and_its_level() {
        let mut duplex = streaming();
        let floor = quiet();
        // No onset seen.
        let actions = duplex.on_stt(500, "book a table", true, true, &floor, false);
        assert_eq!(end_of_turn(&actions).attribution, Attribution::Medium);
        // An onset at the caller's level.
        duplex.on_event(1_000, onset(1_000), &floor, false);
        sure(&mut duplex, 1_400);
        let actions = duplex.on_stt(1_500, "for two people", true, true, &floor, false);
        assert_eq!(end_of_turn(&actions).attribution, Attribution::High);
        // An onset 12 dB or more below it.
        let far = DuplexEvent::Onset {
            at: 2_000,
            level_db: -45.0,
            reflex_ducked: false,
        };
        duplex.on_event(2_000, far, &floor, false);
        sure(&mut duplex, 2_400);
        let actions = duplex.on_stt(2_500, "dinner is ready", true, true, &floor, false);
        assert_eq!(end_of_turn(&actions).attribution, Attribution::Medium);
        // A soft onset of speech at the caller's level: the level of the speech counts, as
        // for a barge-in, so a quiet first sound does not keep a spoken yes from binding.
        duplex.on_event(2_600, far, &floor, false);
        let level = DuplexEvent::Level { level_db: -31.0 };
        duplex.on_event(2_730, level, &floor, false);
        sure(&mut duplex, 2_850);
        let actions = duplex.on_stt(2_900, "yes", true, true, &floor, false);
        assert_eq!(end_of_turn(&actions).attribution, Attribution::High);
        // Typed.
        let actions = duplex.on_typed(3_000, " at eight ");
        let turn = end_of_turn(&actions);
        assert_eq!(
            (turn.text.as_str(), turn.source),
            ("at eight", EotSource::Typed)
        );
        assert_eq!(
            (turn.attribution, turn.turn),
            (Attribution::Medium, TurnId(5))
        );
    }

    #[test]
    fn a_reflex_duck_that_opens_no_barge_in_is_released() {
        // The agent's last words are still playing at the client, but the floor has already
        // closed the utterance: the analyser ducked, and nothing else would undo it.
        let mut duplex = streaming();
        assert_eq!(
            duplex.on_event(0, onset(0), &quiet(), false),
            vec![Gate(GateCmd::Resume), UserSpeaking(true)]
        );
        // Barge-in switched off: the same, with the agent in mid-sentence.
        let off = DuplexConfig {
            barge_in: false,
            ..DuplexConfig::default()
        };
        let mut duplex = Duplex::new(off, lexicon(), false);
        assert_eq!(
            duplex.on_event(1_000, onset(1_000), &agent_speaking(1_000), false),
            vec![Gate(GateCmd::Resume), UserSpeaking(true)]
        );
        // A barge-in that opens keeps the duck: its verdict releases it.
        let mut duplex = streaming();
        assert_eq!(
            duplex.on_event(1_000, onset(1_000), &agent_speaking(1_000), false),
            vec![]
        );
    }

    #[test]
    fn with_no_analyser_the_first_words_are_the_onset() {
        let floor = quiet();
        // The analyser is there and saw nothing: the words are not placed.
        let mut duplex = streaming();
        let actions = duplex.on_stt(1_000, "yes go ahead", true, true, &floor, false);
        let turn = end_of_turn(&actions);
        assert_eq!(
            (turn.onset_at, turn.attribution),
            (None, Attribution::Medium)
        );

        // It cannot run: a turn begins with its first words, so a spoken yes can bind.
        let mut duplex = streaming();
        let degraded = DuplexEvent::Degraded("interaction_missing_build_features");
        duplex.on_event(0, degraded, &floor, false);
        duplex.on_stt(4_000, "yes", false, false, &floor, false);
        let actions = duplex.on_stt(4_600, "yes go ahead", true, true, &floor, false);
        let turn = end_of_turn(&actions);
        assert_eq!(
            (turn.onset_at, turn.attribution),
            (Some(4_000), Attribution::High)
        );
    }

    #[test]
    fn audio_end_ends_the_turn_through_the_flush_that_keeps_the_callbacks() {
        let floor = quiet();
        // Request-response: the buffer is uploaded now, and its final is E.
        let mut duplex = uploading();
        let upload = vec![DuplexAction::FinalizeStt { upload: true }];
        assert_eq!(duplex.on_audio_end(1_000), upload);
        // A second `audio_end` while the upload is away asks for nothing.
        assert_eq!(duplex.on_audio_end(1_100), vec![]);
        let actions = duplex.on_stt(1_800, "book a table", true, true, &floor, false);
        assert_eq!(end_of_turn(&actions).text, "book a table");

        // Nothing was buffered: the empty final the actor sends ends the turn at once, with
        // no words and not as a speech-to-text failure.
        assert_eq!(duplex.on_audio_end(5_000), upload);
        let actions = duplex.on_stt(5_010, "", true, true, &floor, false);
        let turn = end_of_turn(&actions);
        assert_eq!(
            (turn.text.as_str(), turn.source),
            ("", EotSource::SttSpeechFinal)
        );
        assert_eq!(duplex.next_wakeup(), None);

        // Streaming: the words so far are claimed; with none there is nothing to end.
        let mut duplex = streaming();
        assert_eq!(duplex.on_audio_end(1_000), vec![]);
        duplex.on_stt(2_000, "book a table", true, false, &floor, false);
        assert_eq!(duplex.on_audio_end(2_100), vec![DuplexAction::ClaimSegment]);
    }

    #[test]
    fn a_streaming_turn_with_no_words_closes_at_the_silence_ceiling() {
        let mut duplex = streaming();
        let floor = quiet();
        assert_eq!(
            duplex.on_event(0, onset_alone(0), &floor, false),
            vec![UserSpeaking(true)]
        );
        duplex.on_event(
            400,
            DuplexEvent::Offset {
                at: 400,
                voiced_ms: 400,
            },
            &floor,
            false,
        );
        let silence = DuplexEvent::Silence {
            at: 1_900,
            ms: 1_500,
        };
        assert_eq!(
            duplex.on_event(1_900, silence, &floor, false),
            vec![UserSpeaking(false)]
        );

        // With words and no final of its own, the ceiling claims the segment; a claim the
        // speech-to-text never answers ends the turn with the words it had.
        duplex.on_event(5_000, onset(5_000), &floor, false);
        duplex.on_stt(5_300, "book a table", false, false, &floor, false);
        duplex.on_event(
            5_400,
            DuplexEvent::Offset {
                at: 5_400,
                voiced_ms: 400,
            },
            &floor,
            false,
        );
        let silence = DuplexEvent::Silence {
            at: 6_900,
            ms: 1_500,
        };
        assert_eq!(
            duplex.on_event(6_900, silence, &floor, false),
            vec![DuplexAction::ClaimSegment]
        );
        let actions = duplex.tick(9_900, &floor, false);
        let turn = end_of_turn(&actions);
        assert_eq!(
            (turn.text.as_str(), turn.source),
            ("book a table", EotSource::SilenceCeiling)
        );
    }

    /// Deepgram held a finished English sentence as interim words for 4-5 s (2026-10-15
    /// matrix): with a speech-to-text the claim makes finish its words (`Finalize`), interim
    /// words are claimed once Smart Turn is sure and the session's endpointing silence has
    /// passed, not at the silence ceiling.
    #[test]
    fn interim_words_are_claimed_when_smart_turn_is_sure_if_the_claim_finishes_them() {
        let floor = quiet();
        let claimed = |duplex: &mut Duplex| {
            duplex.on_event(1_000, onset(1_000), &floor, false);
            duplex.on_stt(
                2_000,
                "is there a table for two",
                false,
                false,
                &floor,
                false,
            );
            duplex.on_event(
                2_500,
                DuplexEvent::Offset {
                    at: 2_500,
                    voiced_ms: 1_500,
                },
                &floor,
                false,
            );
            let silence = DuplexEvent::Silence { at: 2_700, ms: 200 };
            let mut actions = duplex.on_event(2_700, silence, &floor, false);
            actions.extend(sure(duplex, 2_800));
            actions.contains(&DuplexAction::ClaimSegment)
        };
        assert!(claimed(&mut streaming().with_turn_end_on_claim(Some(200))));
        // No such message: the ceiling still claims them.
        assert!(!claimed(&mut streaming()));
        // A session that asked its speech-to-text for a longer endpointing (a caller who
        // pauses mid-sentence, S3) is not claimed before that silence.
        assert!(!claimed(
            &mut streaming().with_turn_end_on_claim(Some(1_400))
        ));
        // A provider with its own turn model ends its turns itself.
        assert!(!claimed(
            &mut streaming()
                .with_turn_end_on_claim(Some(200))
                .with_provider_turns(true)
        ));
        // Words that dangle are not claimed early, however sure Smart Turn is (live take 24:
        // "... the download numbers in the" was claimed at a 200 ms gap, "terminal?" became a
        // turn of its own).
        let mut duplex = streaming().with_turn_end_on_claim(Some(200));
        duplex.on_event(1_000, onset(1_000), &floor, false);
        duplex.on_stt(
            2_000,
            "the download numbers in the",
            false,
            false,
            &floor,
            false,
        );
        duplex.on_event(
            2_500,
            DuplexEvent::Offset {
                at: 2_500,
                voiced_ms: 1_500,
            },
            &floor,
            false,
        );
        let silence = DuplexEvent::Silence { at: 2_700, ms: 200 };
        let mut actions = duplex.on_event(2_700, silence, &floor, false);
        actions.extend(sure(&mut duplex, 2_800));
        assert!(
            !actions.contains(&DuplexAction::ClaimSegment),
            "{actions:?}"
        );
        // The claim's final (Finalize's, a speech-final) is E.
        let mut duplex = streaming().with_turn_end_on_claim(Some(200));
        assert!(claimed(&mut duplex));
        let actions = duplex.on_stt(
            3_000,
            "Is there a table for two?",
            true,
            true,
            &floor,
            false,
        );
        assert_eq!(end_of_turn(&actions).text, "Is there a table for two?");
    }

    /// A turn that ended on a claim the speech-to-text never answered: the late results that
    /// repeat its words (an interim, then the final) are no second turn. Other words are.
    #[test]
    fn the_late_answer_to_an_unanswered_claim_is_no_turn_of_its_own() {
        let floor = quiet();
        let mut duplex = streaming();
        duplex.on_event(5_000, onset(5_000), &floor, false);
        duplex.on_stt(5_300, "book a table", false, false, &floor, false);
        duplex.on_event(
            5_400,
            DuplexEvent::Offset {
                at: 5_400,
                voiced_ms: 400,
            },
            &floor,
            false,
        );
        let silence = DuplexEvent::Silence {
            at: 6_900,
            ms: 1_500,
        };
        assert_eq!(
            duplex.on_event(6_900, silence, &floor, false),
            vec![DuplexAction::ClaimSegment]
        );
        end_of_turn(&duplex.tick(9_900, &floor, false));
        assert_eq!(
            duplex.on_stt(10_000, "book a table", false, false, &floor, false),
            vec![]
        );
        assert_eq!(
            duplex.on_stt(10_900, "Book a table.", true, true, &floor, false),
            vec![]
        );
        // Then the caller's next words are a turn as before.
        let actions = duplex.on_stt(12_000, "for two", true, true, &floor, false);
        assert_eq!(end_of_turn(&actions).text, "for two");
    }

    #[test]
    fn words_with_no_onset_while_echo_is_heard_are_the_agents_own() {
        let floor = agent_speaking(1_200);
        let mut duplex = streaming();
        let echo = DuplexEvent::Echo { at: 1_000 };
        assert_eq!(duplex.on_event(1_000, echo, &floor, false), vec![]);
        // The speech-to-text hears the echo; the analyser saw no onset of the caller's.
        // Nothing is ducked, and the turn is not the caller's, however short it is.
        assert_eq!(
            duplex.on_stt(1_200, "four hundred", false, false, &floor, false),
            vec![]
        );
        assert_eq!(duplex.next_wakeup(), None, "no barge-in is open");
        let actions = duplex.on_stt(1_900, "four hundred", true, true, &floor, false);
        let turn = end_of_turn(&actions);
        assert!(turn.suspected_echo);
        assert_eq!(
            (turn.class, turn.attribution, turn.onset_at),
            (Class::Echo, Attribution::Low, None)
        );
        assert_eq!(actions.len(), 1, "{actions:?}");

        // Two seconds after the last echo, words with no onset are placed as before.
        assert_eq!(
            duplex.on_stt(3_001, "four hundred", false, false, &floor, false),
            vec![Gate(GateCmd::Duck), asked(2, "four hundred")]
        );
    }

    #[test]
    fn a_caller_over_the_echo_is_judged_on_the_words_that_are_not_the_agents() {
        let hears_echo = |duplex: &mut Duplex, at: Ms| {
            let echo = DuplexEvent::Echo { at };
            assert_eq!(
                duplex.on_event(at, echo, &agent_speaking(at), false),
                vec![]
            );
            let heard = "I found two flights";
            duplex.on_stt(at + 50, heard, false, false, &agent_speaking(at), false)
        };

        // An onset that passed the echo defence opens the barge-in. The transcript holds
        // the agent's words and the caller's: the caller's stop cue cuts.
        let mut duplex = streaming();
        assert_eq!(hears_echo(&mut duplex, 900), vec![]);
        let floor = agent_speaking(1_300);
        assert_eq!(duplex.on_event(1_000, onset(1_000), &floor, false), vec![]);
        let heard = "I found two flights under four hundred stop";
        assert_eq!(
            duplex.on_stt(1_300, heard, false, false, &floor, false),
            vec![asked(1, "stop")]
        );
        assert_eq!(verdict(&mut duplex, 1_350, Class::Interrupt), interrupt());
        // The turn has an onset of the caller's: its words are judged by the transcript
        // filter alone.
        let actions = duplex.on_stt(2_000, "stop please stop", true, true, &quiet(), false);
        assert!(!end_of_turn(&actions).suspected_echo);

        // With only the agent's words the sound is judged as a sound: still voiced at the
        // verdict, it interrupts. Before, the agent's words made it echo and it resumed.
        let mut duplex = streaming();
        assert_eq!(hears_echo(&mut duplex, 900), vec![]);
        assert_eq!(duplex.on_event(1_000, onset(1_000), &floor, false), vec![]);
        let heard = "I found two flights under four hundred dollars";
        assert_eq!(
            duplex.on_stt(1_300, heard, false, false, &floor, false),
            vec![]
        );
        assert_eq!(
            duplex.tick(1_400, &floor, false),
            vec![Gate(GateCmd::Pause)]
        );
        assert_eq!(
            duplex.tick(1_900, &agent_speaking(1_900), false),
            interrupt()
        );
    }

    #[test]
    fn on_a_phone_line_with_no_canceller_the_first_words_open_the_barge_in() {
        let floor = agent_speaking(1_000);
        let mut duplex = streaming();
        let words_first = DuplexEvent::WordsFirst(true);
        assert_eq!(duplex.on_event(0, words_first, &quiet(), false), vec![]);
        // A sound over the agent may be the agent's own: nothing is ducked or paused.
        assert_eq!(
            duplex.on_event(1_000, onset_alone(1_000), &floor, false),
            vec![]
        );
        assert_eq!(duplex.next_wakeup(), None);
        assert_eq!(duplex.tick(1_900, &agent_speaking(1_900), false), vec![]);
        // The words open the barge-in and decide it at once.
        let floor = agent_speaking(2_000);
        assert_eq!(
            duplex.on_stt(2_000, "hold on", false, false, &floor, false),
            vec![Gate(GateCmd::Duck), asked(1, "hold on")]
        );
        assert_eq!(verdict(&mut duplex, 2_050, Class::Interrupt), interrupt());

        // With the agent silent the caller has the floor at the onset, as on any line.
        let mut duplex = streaming();
        duplex.on_event(0, words_first, &quiet(), false);
        assert_eq!(
            duplex.on_event(1_000, onset_alone(1_000), &quiet(), false),
            vec![UserSpeaking(true)]
        );

        // With the canceller a bare onset opens the barge-in again.
        let mut duplex = streaming();
        duplex.on_event(0, words_first, &quiet(), false);
        let bare = DuplexEvent::WordsFirst(false);
        duplex.on_event(0, bare, &quiet(), false);
        assert_eq!(
            duplex.on_event(1_000, onset_alone(1_000), &floor, false),
            vec![Gate(GateCmd::Duck)]
        );
    }

    #[test]
    fn the_end_of_an_echo_segment_does_not_close_a_callers_barge_in() {
        let floor = agent_speaking(5_000);
        let mut duplex = streaming();
        let echo = |duplex: &mut Duplex, at: Ms| {
            let event = DuplexEvent::Echo { at };
            assert_eq!(
                duplex.on_event(at, event, &agent_speaking(at), false),
                vec![]
            );
        };
        echo(&mut duplex, 3_000);
        assert_eq!(
            duplex.on_stt(3_100, "I found two flights", false, false, &floor, false),
            vec![]
        );
        // The echo is silence to the analyser: its silence reports claim no segment.
        echo(&mut duplex, 4_400);
        let silence = DuplexEvent::Silence {
            at: 4_500,
            ms: 1_500,
        };
        assert_eq!(duplex.on_event(4_500, silence, &floor, false), vec![]);
        // The caller cuts in; then the vendor ends the echo's segment.
        assert_eq!(duplex.on_event(5_000, onset(5_000), &floor, false), vec![]);
        echo(&mut duplex, 5_100);
        let heard = "I found two flights under four hundred dollars";
        let actions = duplex.on_stt(5_300, heard, true, true, &floor, false);
        assert!(end_of_turn(&actions).suspected_echo);
        assert!(!actions.contains(&classified(Class::Noise)), "{actions:?}");
        // The barge-in runs on: paused, and at the verdict the caller still speaks.
        assert_eq!(
            duplex.tick(5_400, &floor, false),
            vec![Gate(GateCmd::Pause)]
        );
        assert_eq!(
            duplex.tick(5_900, &agent_speaking(5_900), false),
            vec![classified(Class::Interrupt), Cut(CutCause::Interrupt)]
        );
        // What the caller says next is the caller's, though echo was heard a second ago.
        let actions = duplex.on_stt(6_200, "hold on a second", false, false, &quiet(), false);
        assert_eq!(actions, vec![UserSpeaking(true)]);
        let actions = duplex.on_stt(7_000, "hold on a second", true, true, &quiet(), false);
        assert!(!end_of_turn(&actions).suspected_echo);
    }
}

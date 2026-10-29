//! The health watcher of one session (plan chapter 10, "When services fail"): which service
//! the call depends on is failing, and which pre-made line plays when. Pure: the actor feeds
//! it the health signals of the code paths that already report them (the recogniser's errors
//! and silence while the caller speaks, the front supervisor, the background run's first
//! token, the synthesis' errors and its stall) and asks it at every step what to play. It is
//! never fed the caller's words.
//!
//! - **Stall** - a failing service whose stall is due gets a `stall` line (`hearing` for the
//!   recogniser) at the next slot: the caller is silent and no agent audio plays. Further
//!   ones at most every [`STALL_GAP_MS`], at most [`MAX_STALLS`] in one outage, rotated.
//! - **Back** - when every service is well again, a `back` line plays if a stall was heard.
//! - **Exit** - `max_wait` after the first stall with a service that ends the call still
//!   failing, or at once when a service the whole session needs is gone: the `exit` line. An
//!   outage that starts again within `max_wait` of the last one's last stall line is the same
//!   outage: its first stall counts, so a service that keeps failing and coming back still
//!   ends the call.

use super::types::Ms;

/// The least time between two stall lines.
pub const STALL_GAP_MS: Ms = 8_000;
/// The most stall lines in one outage.
pub const MAX_STALLS: u32 = 3;
/// A streaming recogniser that gives no result for this much of the caller's speech fails.
pub const UNHEARD_SPEECH_MS: u32 = 3_000;

/// A service the call depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    /// The speech-to-text.
    Recogniser,
    /// The front model.
    Front,
    /// The background LLM or the orchestrator.
    Background,
    /// The text-to-speech.
    Voice,
}

impl Service {
    /// The name on the wire (`session_failed.service`, `interaction_updated.failure`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Recogniser => "recogniser",
            Self::Front => "front",
            Self::Background => "background",
            Self::Voice => "voice",
        }
    }

    /// The `degraded` code while it fails.
    pub fn degraded(self) -> &'static str {
        match self {
            Self::Recogniser => "recogniser_unavailable",
            Self::Front => "front_unavailable",
            Self::Background => "background_stalled",
            Self::Voice => "voice_unavailable",
        }
    }

    /// Whether its outage ends the call after `max_wait`. The front's does not: a front that
    /// is not back within the supervisor's restart window ends interaction mode instead
    /// (chapter 9), or the call when the session has no plain path (`Watcher::gone`).
    fn exits(self) -> bool {
        self != Self::Front
    }
}

/// The kinds of the pack's `failure` lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineKind {
    Stall,
    Hearing,
    Exit,
    /// The exit line that promises to get back: only with a follow-up channel.
    ExitFollowUp,
    Back,
}

impl LineKind {
    pub const ALL: [LineKind; 5] = [
        Self::Stall,
        Self::Hearing,
        Self::Exit,
        Self::ExitFollowUp,
        Self::Back,
    ];

    /// The line's `subsituation` in the pack, and its `kind` on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stall => "stall",
            Self::Hearing => "hearing",
            Self::Exit => "exit",
            Self::ExitFollowUp => "exit_follow_up",
            Self::Back => "back",
        }
    }

    pub fn parse(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == word)
    }
}

/// What may start now. `stall`: the caller is silent and nothing of the agent's plays.
/// `back`: the caller is silent and no line of the agent's plays (a stall line may: the back
/// line queues behind it, ahead of the answer). `caller_silent`: the exit waits for nothing
/// else (it ends the call).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub stall: bool,
    pub back: bool,
    pub caller_silent: bool,
}

impl Slot {
    /// Everything may start.
    pub const OPEN: Slot = Slot {
        stall: true,
        back: true,
        caller_silent: true,
    };
    /// The caller speaks.
    pub const SHUT: Slot = Slot {
        stall: false,
        back: false,
        caller_silent: false,
    };
}

/// A line to play now: its kind, the service it is about, and the rotation index within the
/// kind (the `n`th line of that kind this session).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Line {
    pub kind: LineKind,
    pub service: Service,
    pub n: usize,
}

#[derive(Debug, Clone, Copy)]
struct Failing {
    service: Service,
    /// From when a stall may play for it.
    due: Ms,
}

#[derive(Debug, Clone)]
pub struct Watcher {
    max_wait_ms: Ms,
    /// A follow-up channel exists: the exit line may promise to get back.
    follow_up: bool,
    failing: Vec<Failing>,
    /// The stall lines of this outage, the last one's time, the first one's time.
    stalls: u32,
    last_stall: Option<Ms>,
    first_stall: Option<Ms>,
    /// The service of the last stall line: the back line is about it.
    stalled: Option<Service>,
    /// The outage before this one, once it was over: its first and its last stall line. An
    /// outage within `max_wait` of that last line continues it (it flapped back).
    previous: Option<(Ms, Ms)>,
    /// A service the whole session needs is gone.
    gone: Option<Service>,
    /// The exit line was given.
    exited: bool,
    /// How many lines of each kind played this session (the rotation).
    played: [usize; 5],
}

impl Watcher {
    pub fn new(max_wait_s: u32, follow_up: bool) -> Self {
        Watcher {
            max_wait_ms: Ms::from(max_wait_s) * 1_000,
            follow_up,
            failing: Vec::new(),
            stalls: 0,
            last_stall: None,
            first_stall: None,
            stalled: None,
            previous: None,
            gone: None,
            exited: false,
            played: [0; 5],
        }
    }

    /// `service` fails; a stall for it may play from `due` on (a deadline that passed: now; an
    /// error: after the stall time, so a single error that the next attempt mends plays
    /// nothing). Returns `true` when it was well until now.
    pub fn failing(&mut self, service: Service, due: Ms) -> bool {
        if self
            .failing
            .iter()
            .any(|failing| failing.service == service)
        {
            return false;
        }
        self.failing.push(Failing { service, due });
        true
    }

    /// `service` works again. Returns `true` when it was failing.
    pub fn recovered(&mut self, service: Service) -> bool {
        let before = self.failing.len();
        self.failing.retain(|failing| failing.service != service);
        let was = self.failing.len() < before;
        if self.failing.is_empty() && self.stalls == 0 {
            self.first_stall = None;
        }
        was
    }

    pub fn is_failing(&self, service: Service) -> bool {
        self.failing
            .iter()
            .any(|failing| failing.service == service)
    }

    /// The services failing now, oldest first.
    pub fn failing_services(&self) -> Vec<Service> {
        self.failing.iter().map(|failing| failing.service).collect()
    }

    /// A service the whole session needs will not come back: the exit is due at once.
    pub fn gone(&mut self, service: Service) {
        self.gone.get_or_insert(service);
    }

    /// The exit line was given.
    pub fn exited(&self) -> bool {
        self.exited
    }

    /// A back line is owed: a stall was heard and nothing fails now.
    pub fn back_owed(&self) -> bool {
        self.failing.is_empty() && self.stalls > 0
    }

    /// The agent spoke again before the back line could: it is not said (the caller hears
    /// that the call goes on), and the next outage counts from zero.
    pub fn drop_back(&mut self) {
        if self.back_owed() {
            self.end_outage();
            self.stalled = None;
        }
    }

    /// What plays now, if anything.
    pub fn next(&mut self, now: Ms, slot: Slot) -> Option<Line> {
        if self.exited {
            return None;
        }
        if let Some(service) = self.exit_due(now)
            && slot.caller_silent
        {
            self.exited = true;
            let kind = if self.follow_up {
                LineKind::ExitFollowUp
            } else {
                LineKind::Exit
            };
            return Some(self.line(kind, service));
        }
        if self.failing.is_empty() {
            if self.stalls == 0 || !slot.back {
                return None;
            }
            let service = self.stalled.take().unwrap_or(Service::Background);
            self.end_outage();
            return Some(self.line(LineKind::Back, service));
        }
        if !slot.stall {
            return None;
        }
        let spaced = self.last_stall.is_none_or(|at| now >= at + STALL_GAP_MS);
        let due = self.failing.iter().find(|failing| now >= failing.due)?;
        if self.stalls >= MAX_STALLS || !spaced {
            return None;
        }
        let service = due.service;
        self.stalls += 1;
        self.last_stall = Some(now);
        if self.first_stall.is_none() {
            self.first_stall = Some(self.outage_start(now));
        }
        self.stalled = Some(service);
        let kind = if service == Service::Recogniser {
            LineKind::Hearing
        } else {
            LineKind::Stall
        };
        Some(self.line(kind, service))
    }

    /// This outage is over (heard back, or the agent spoke again): its stall lines are kept
    /// as the previous outage, and the next one starts its own count unless it comes soon.
    fn end_outage(&mut self) {
        if let (Some(first), Some(last)) = (self.first_stall, self.last_stall) {
            self.previous = Some((first, last));
        }
        self.stalls = 0;
        self.last_stall = None;
        self.first_stall = None;
    }

    /// The first stall of the outage whose first line plays `now`: the previous outage's,
    /// when this one started within `max_wait` of its last line (spec 029 W-6), else `now`.
    fn outage_start(&self, now: Ms) -> Ms {
        match self.previous {
            Some((first, last)) if now < last + self.max_wait_ms => first,
            _ => now,
        }
    }

    /// The service whose outage ends the call now.
    fn exit_due(&self, now: Ms) -> Option<Service> {
        if let Some(service) = self.gone {
            return Some(service);
        }
        let first = self.first_stall?;
        let exiting = self
            .failing
            .iter()
            .find(|failing| failing.service.exits())?;
        (now >= first + self.max_wait_ms).then_some(exiting.service)
    }

    fn line(&mut self, kind: LineKind, service: Service) -> Line {
        let slot = LineKind::ALL.iter().position(|k| *k == kind).unwrap_or(0);
        let n = self.played[slot];
        self.played[slot] += 1;
        Line { kind, service, n }
    }

    /// The next time [`Self::next`] may have something new to play.
    pub fn next_wakeup(&self) -> Option<Ms> {
        if self.exited {
            return None;
        }
        let exit = self
            .first_stall
            .filter(|_| self.failing.iter().any(|f| f.service.exits()))
            .map(|first| first + self.max_wait_ms);
        let stall = (self.stalls < MAX_STALLS)
            .then(|| self.failing.iter().map(|failing| failing.due).min())
            .flatten()
            .map(|due| due.max(self.last_stall.map_or(0, |at| at + STALL_GAP_MS)));
        [exit, stall].into_iter().flatten().min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_exit(line: &Line) -> bool {
        matches!(line.kind, LineKind::Exit | LineKind::ExitFollowUp)
    }

    /// Spec 029 W-6: a service that fails and comes back every few seconds is one outage, not many.
    /// The call ends `max_wait` after the first stall, however often the service flaps; before,
    /// each "back" started the count again and the caller heard stall and back lines forever.
    #[test]
    fn an_outage_that_recurs_within_max_wait_counts_toward_the_same_exit() {
        let mut watcher = Watcher::new(30, false);
        let mut exit = None;
        let mut at: Ms = 0;
        while at < 120_000 && exit.is_none() {
            // Fails for 4 s, is heard back, works for 6 s, and fails again.
            watcher.failing(Service::Background, at);
            for t in (at..at + 4_000).step_by(500) {
                if watcher
                    .next(t, Slot::OPEN)
                    .is_some_and(|line| is_exit(&line))
                {
                    exit = Some(t);
                    break;
                }
            }
            if exit.is_some() {
                break;
            }
            watcher.recovered(Service::Background);
            for t in (at + 4_000..at + 10_000).step_by(500) {
                let _ = watcher.next(t, Slot::OPEN);
            }
            at += 10_000;
        }
        let exit = exit.expect("a flapping background ends the call");
        assert!(
            (30_000..=34_000).contains(&exit),
            "max_wait after the first stall, not later: {exit}"
        );
    }

    /// The other side of spec 029 W-6: an outage long after the last one (more than `max_wait`
    /// after its last stall line) is a new one, with its own `max_wait`.
    #[test]
    fn an_outage_long_after_the_last_one_starts_its_own_count() {
        let mut watcher = Watcher::new(30, false);
        watcher.failing(Service::Background, 0);
        assert_eq!(
            watcher.next(0, Slot::OPEN).map(|l| l.kind),
            Some(LineKind::Stall)
        );
        watcher.recovered(Service::Background);
        assert_eq!(
            watcher.next(1_000, Slot::OPEN).map(|l| l.kind),
            Some(LineKind::Back)
        );
        // 41 s later, more than 30 s after the last stall line.
        watcher.failing(Service::Background, 41_000);
        assert_eq!(
            watcher.next(41_000, Slot::OPEN).map(|l| l.kind),
            Some(LineKind::Stall)
        );
        for t in (41_500..71_000).step_by(500) {
            assert!(
                !watcher
                    .next(t, Slot::OPEN)
                    .is_some_and(|line| is_exit(&line)),
                "no exit before 30 s of this outage: {t}"
            );
        }
        assert!(
            watcher
                .next(71_000, Slot::OPEN)
                .is_some_and(|line| is_exit(&line))
        );
    }

    #[test]
    fn stall_lines_are_spaced_rotated_and_capped() {
        let mut watcher = Watcher::new(30, false);
        assert_eq!(watcher.next(0, Slot::OPEN), None, "nothing fails");
        assert!(watcher.failing(Service::Background, 2_500));
        assert!(
            !watcher.failing(Service::Background, 9_000),
            "already failing"
        );
        assert_eq!(watcher.next(2_000, Slot::OPEN), None, "not due yet");
        assert_eq!(watcher.next_wakeup(), Some(2_500));
        // Never over the caller or the agent: it waits for the slot.
        assert_eq!(watcher.next(2_500, Slot::SHUT), None);
        let first = watcher.next(3_000, Slot::OPEN).expect("the first stall");
        assert_eq!(
            (first.kind, first.service, first.n),
            (LineKind::Stall, Service::Background, 0)
        );
        // 8 s apart.
        assert_eq!(watcher.next(10_999, Slot::OPEN), None);
        assert_eq!(watcher.next_wakeup(), Some(11_000));
        let second = watcher.next(11_000, Slot::OPEN).expect("the second stall");
        assert_eq!(second.n, 1, "rotated");
        let third = watcher.next(19_000, Slot::OPEN).expect("the third stall");
        assert_eq!(third.n, 2);
        // At most three.
        assert_eq!(watcher.next(27_000, Slot::OPEN), None);
        // The exit, 30 s after the first stall.
        assert_eq!(watcher.next_wakeup(), Some(33_000));
        assert_eq!(watcher.next(32_999, Slot::OPEN), None);
        // The exit waits for the caller only.
        assert_eq!(watcher.next(33_000, Slot::SHUT), None);
        let exit = watcher
            .next(
                33_100,
                Slot {
                    caller_silent: true,
                    ..Slot::SHUT
                },
            )
            .expect("the exit");
        assert_eq!(
            (exit.kind, exit.service),
            (LineKind::Exit, Service::Background)
        );
        assert!(watcher.exited());
        assert_eq!(watcher.next(40_000, Slot::OPEN), None, "once");
    }

    #[test]
    fn a_service_that_recovers_gets_the_back_line_only_after_a_stall() {
        let mut watcher = Watcher::new(30, false);
        // Mended before its stall was due: nothing is said.
        watcher.failing(Service::Voice, 2_500);
        assert!(watcher.recovered(Service::Voice));
        assert_eq!(watcher.next(5_000, Slot::OPEN), None);
        // A stall heard, then the recovery: the back line, and a fresh count.
        watcher.failing(Service::Recogniser, 6_000);
        let hearing = watcher.next(6_000, Slot::OPEN).expect("the hearing line");
        assert_eq!(hearing.kind, LineKind::Hearing);
        watcher.recovered(Service::Recogniser);
        assert_eq!(
            watcher.next(
                6_500,
                Slot {
                    caller_silent: true,
                    ..Slot::SHUT
                }
            ),
            None,
            "the back line waits for a slot"
        );
        let back = watcher.next(7_000, Slot::OPEN).expect("the back line");
        assert_eq!(back.kind, LineKind::Back);
        assert_eq!(watcher.next(7_100, Slot::OPEN), None, "once");
        // The next outage counts from zero, and is not 8 s after the last stall.
        watcher.failing(Service::Voice, 8_000);
        assert_eq!(
            watcher.next(8_000, Slot::OPEN).map(|l| l.kind),
            Some(LineKind::Stall)
        );
    }

    #[test]
    fn a_back_line_queues_behind_the_stall_and_is_dropped_once_the_agent_spoke() {
        let mut watcher = Watcher::new(30, false);
        watcher.failing(Service::Background, 0);
        watcher.next(0, Slot::OPEN).expect("a stall");
        watcher.recovered(Service::Background);
        // The stall line still plays: the back line queues behind it, ahead of the answer.
        let behind = Slot {
            stall: false,
            back: true,
            caller_silent: true,
        };
        assert_eq!(
            watcher.next(100, behind).map(|l| l.kind),
            Some(LineKind::Back)
        );
        // Another outage; the answer starts before the back line could.
        watcher.failing(Service::Background, 200);
        watcher.next(200, Slot::OPEN).expect("a stall");
        watcher.recovered(Service::Background);
        assert!(watcher.back_owed());
        watcher.drop_back();
        assert!(!watcher.back_owed());
        assert_eq!(watcher.next(300, Slot::OPEN), None);
    }

    #[test]
    fn the_exit_line_promises_a_follow_up_only_when_there_is_one() {
        for (follow_up, kind) in [(false, LineKind::Exit), (true, LineKind::ExitFollowUp)] {
            let mut watcher = Watcher::new(5, follow_up);
            watcher.failing(Service::Voice, 0);
            watcher.next(0, Slot::OPEN).expect("a stall");
            assert_eq!(watcher.next(5_000, Slot::OPEN).map(|l| l.kind), Some(kind));
        }
    }

    #[test]
    fn the_front_ends_the_call_only_when_it_is_gone() {
        let mut watcher = Watcher::new(5, false);
        watcher.failing(Service::Front, 0);
        watcher.next(0, Slot::OPEN).expect("a stall");
        // Two more stalls at most, and no exit after max_wait.
        let later: Vec<LineKind> = [8_000, 16_000, 60_000]
            .into_iter()
            .filter_map(|at| watcher.next(at, Slot::OPEN).map(|line| line.kind))
            .collect();
        assert_eq!(
            later,
            [LineKind::Stall, LineKind::Stall],
            "no exit after max_wait"
        );
        watcher.gone(Service::Front);
        let exit = watcher
            .next(
                60_100,
                Slot {
                    caller_silent: true,
                    ..Slot::SHUT
                },
            )
            .expect("the exit");
        assert_eq!((exit.kind, exit.service), (LineKind::Exit, Service::Front));
    }
}

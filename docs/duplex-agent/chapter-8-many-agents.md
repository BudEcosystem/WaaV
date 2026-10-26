# Chapter 8. Many tasks, many agents: the orchestrator, the update inbox and follow-ups

Added 2026-10-15 at the product owner's request. It extends chapter 3 (background tasks) and chapter 4 (policy and
human in the loop); where this chapter and those differ, this chapter wins for the items it names.

## What this part is and why it exists

Chapters 3 and 4 give a call one background at a time: one backend per session (`tasks.default_backend`), at most
three live tasks, two queued and fifty per session, and a remote backend (webhook, A2A, MCP) that runs one task at a
time. A finished result reaches the client only after it was spoken (`task_result` carries `spoken`), and it is
spoken in the words the background wrote, whatever the conversation has moved on to.

The experience this plan copies works differently. A caller can say "book me a flight to Delhi on Friday", then "and
fix the failing test in my repo", then ask "what's the capital of Peru?". The agent answers the question at once, a
screen-operating agent books the flight and a coding agent fixes the test, all at the same time. When the flight is
booked the agent finishes what it is saying and then, briefly and in the context of the conversation, says so ("by the
way, your Friday flight is booked, 9:40 am"). A screen shows the full result the moment it exists. If the airline needs
an answer within five seconds, the agent breaks in politely to ask, then goes back to what it was saying. And the
caller can ask about a finished job ("when does it land?") or act on it ("book the return too") and the same agent
carries on with its context.

This chapter adds that. Five requirements from the product owner, and the gaps found while writing it:

1. **No fixed limit on work.** The gateway takes any number of tasks. A large orchestrator model plans and dispatches
   them; the small front model keeps giving immediate feedback.
2. **Many agents at once.** One call uses several agents, concurrently, and the same agent for several tasks.
3. **Updates fitted into the conversation, and published at once.** The agent never interrupts its own answer or the
   caller for a result. It finishes, then gives a short update if the caller wants it. Every result is published to
   the client and to an endpoint the moment it exists, independent of speech.
4. **Questions with a deadline interrupt.** A job's question that expires before a natural pause would come breaks in at
   the next clause boundary, and the interrupted answer resumes afterwards.
5. **Results are reworded for the moment, and follow-ups reach the same job.** The front model turns a result into a
   very short spoken update that fits what is being talked about. A question or an action about a finished job goes to
   that job's agent with its context, or is answered from the stored result.

## How it works

### The orchestrator

`tasks.orchestrator` names a model for the LLM tier (chapter 3's run driver, `core/interaction/background.rs`), normally
a large one. Its tools are the session's agents, plus the session's ordinary tools:

- **The agent catalog.** Each entry of the operator's `interaction.task_backends` gains `description` (one sentence the
  orchestrator reads: "Operates websites in a browser: bookings, forms, purchases") and `max_concurrent` (default: no
  limit for webhook, A2A, MCP and Responses entries; a Bud agent entry runs each task in its own agent conversation, so
  it too runs in parallel). A session lists the entries it may use with `tasks.agents` (names only; addresses and
  secrets stay on the server, as today).
- **Dispatch never blocks.** Calling an agent tool starts a **child task** on that agent through the existing driver
  (`remote.rs` for webhook, A2A, MCP and Responses; `bud_agent.rs` for Bud agents) and returns at once with the child's
  `task_ref`. The orchestrator may call several agents in one round. It hears each child's result as a later message in
  its own history and can then dispatch the next step ("after the flight is booked, book a hotel near the airport"), so
  dependencies need no new gateway mechanism.
- **General questions** are answered by the orchestrator itself after its `ANSWER:` marker, streamed sentence by
  sentence as today. Short social and status turns stay with the front (chapter 2).
- **One caller utterance is still one task operation for the gateway** (chapter 3): it goes to the orchestrator once.
  The orchestrator splits a multi-part request ("book the flight and fix the test") into children.
- Without `tasks.orchestrator`, a session runs as today, on `tasks.default_backend`.

### A standalone WaaV agent that calls agents over A2A and MCP

The orchestrator is **WaaV's own agent**, part of the gateway, and needs nothing from budprompt (product owner,
2026-10-16). It calls other agents as tools in two standard formats; a Bud agent, or any other, is reached the same way
when it exposes either one. The signed webhook stays for operators who have neither.

- **A2A 1.0.** An entry is the agent's base URL. At session start (cached per server for `catalog_ttl_s`, default
  300 s) the gateway reads its **Agent Card** (`/.well-known/agent-card.json`): the card's description and skills
  become the orchestrator tool's description, and its capabilities say whether it streams. A call is `message/send`, or
  `message/stream` when the card says it streams, so progress and `input-required` questions arrive as they happen;
  `tasks/get` and `tasks/cancel` as today; the task's `contextId` is the follow-up handle (below). The card's security
  scheme decides the credential the gateway sends (bearer token from `secret_env`).
- **MCP (2026-07-28).** An entry is the server's URL. The gateway runs `initialize` and `tools/list`, and **every tool
  the operator allows** (`tools: ["*"]` or a list) becomes its own orchestrator tool with the server's name, description
  and input JSON schema. A call is `tools/call`, as an MCP Task when the server supports tasks (`tasks/get`,
  `tasks/cancel`); elicitation requests become questions for the caller (chapter 4, and chapter 4's screen forms);
  progress notifications become `task_progress`.
- **Confirmations without budprompt.** Whether a call must be confirmed with the caller comes from the gateway, not the
  agent's author: the entry's `side_effects` (per agent), and for MCP the tool's annotations (`readOnlyHint`,
  `destructiveHint`) unless the operator overrides them. A side-effecting call goes through the existing gate of
  chapter 3: the caller is asked from the call's exact arguments before it is sent, and a correction refuses a stale
  call.
- **Every agent call is a child task** with its own state, effects and trace id, whichever protocol carries it.

This replaces the Bud-agent items earlier chapters left blocked on budprompt (a conversation per task, the agent
author's confirmation templates): an agent that needs them exposes A2A or MCP.

The task tree is visible on the wire: every task event gains `parent_id` (null for a top-level task) and `agent` (the
catalog entry's name). A status question ("what are you working on?") is answered from the tree: the three most recent
top-level tasks by title, and how many more.

### Limits become settings

The fixed caps (`MAX_ACTIVE` 3, `MAX_QUEUED` 2, `MAX_PER_SESSION` 50 in `core/interaction/tasks.rs`) become operator
settings under `interaction.tasks.limits` with no gateway limit by default: `max_live` per session, `max_live_tenant`
per tenant, `max_per_agent` from the catalog, and a per-tenant spend cap where the backend reports tokens. Work beyond a
limit **waits in a queue**; it is never refused for count. The caller hears "that's queued behind three others". A
finished task leaves the live count at once and stays addressable for follow-ups (below).

### The update inbox

Every task event (result, failure, progress, question) enters a per-session **update inbox** ordered by priority
(question with a deadline, then failures that need the caller, then results, then progress) and age. Two things happen
independently:

- **Published at once.** At completion the gateway sends a new `task_output` event with the full result (text,
  structured `data`, `artifacts` such as links or files the agent returned, `effects`) to the session's socket and the
  LiveKit data topic, and on a new server-sent-events endpoint `GET /v1/tasks/events` (tenant-authenticated like
  `GET /v1/tasks`; filter by `stream_id`, `task_id` or `parent_id`). A UI can show the result while the agent is still
  talking about something else. The existing `task_result` keeps its meaning (sent once the spoken part is done, with
  `spoken`), so current clients are unaffected. A client may send `task_seen {task_id}` when the caller has read the
  result on screen; a spoken update for it is then dropped.
- **Spoken when it fits.** The policy (chapter 4) speaks an update only at a **slot**: the agent's current answer has
  finished (its relay is closed) and the caller does not hold the floor. Updates never cut the agent's own answer and
  never start over the caller. At a slot:
  - **Idle conversation** (no caller speech for 4 s and nothing else queued): the update is spoken.
  - **Active conversation**: `tasks.announce` decides. `ask` (default) appends one short offer to the end of the
    answer ("by the way, your flight update is in; want it now?"). Yes plays it, no or no reply keeps it for the next
    idle slot or a later "what about the flight?". `tell` appends the short update itself. `screen` never speaks it
    unless asked.
  - **The caller's own instruction wins** for that task: "tell me as soon as the build finishes" makes it `tell` and
    raises its priority; "don't tell me about it" makes it `screen`.
  - **Several updates at one slot** become one offer ("two updates are in: the flight and the test fix").
  - Progress is spoken only for a task the caller is waiting on (the conversation has nothing else going), with chapter
    4's spacing rules.

### Rewording: the narrator

The front model (the small local model of chapter 2, or the scripted front) writes every spoken update. It is given the
last two turns of the conversation, the task's title, and the result's summary, and asked for one sentence of at most
20 words that fits what is being talked about. Nothing it says may go beyond the result: the outbound speech policy's
check that numbers, names and links were given (chapter 3) gains the task's result as a source, and a failed check
falls back to the template `Your {title} is done: {first sentence of the summary}`. The same template is used when the
front has not answered within 400 ms. When a screen is attached the update ends with "the details are on your screen"
instead of reading them out. The background's full answer is still available: "tell me more" relays it as today.

### Questions with a deadline

A job's question may carry a deadline: `expires_in_ms` on the webhook and A2A wires (an A2A message metadata field), an
MCP elicitation's `_meta`, or the agent entry's `question_deadline_ms` default. The policy computes whether the
question can wait for a natural slot (the expected end of the current answer plus about 2.5 s to ask). If it cannot:

- **While the agent speaks:** the question takes the floor at the next clause boundary (chapter 4's rank cut, with a
  rank above the reply), introduced by one short phrase from the persona ("sorry, one quick thing from the airline").
  After the caller answers, the **interrupted answer resumes** from the first sentence the caller did not hear (the
  floor's heard timeline knows it), introduced by "anyway," or the persona's equivalent.
- **While the caller speaks:** it waits for the caller's first pause of at least 250 ms (the analyser's pause signal),
  and never starts over the caller.
- **On a screen** it appears at once, with the remaining time.
- **When it expires:** the job gets `timeout` (as today), and the caller hears one line at the next slot about what the
  job did ("the airline needed the seat choice, so it picked one").

A question that can wait is queued in the inbox like an update and asked at the next slot.

### Follow-ups

A finished task stays addressable for `tasks.follow_up_ttl_s` (default 1,800 s), with its result and its backend's
continuation handle: the LLM tier's history key, the A2A `contextId`, the Bud agent conversation id, the webhook
`task_id`, the MCP task id, or the Responses `previous_response_id`.

- The front's act gains `follow_up` (a task named by its title, or by default the task whose update was spoken last).
- **A question the result answers** ("what time does it land?") is answered by the narrator from the stored result,
  with the same checks as an update.
- **Anything else** ("can it be a window seat?", "book the return too", "cancel it") becomes a new child task on the
  same agent with its continuation handle, so the agent keeps its context. Cancelling something already done is a new
  compensating task and is confirmed first (chapter 4).
- If the handle has expired, the new task's request carries a one-line summary of the old result instead.

### Other gaps closed here

- **A hesitant request stays one task.** Words that continue a request within the eager window after a pause (the
  experience evaluation's disfluent requests) revise the provisional task instead of creating another.
- **Per-agent health.** Each catalog entry gets its own failure breaker, timeout and one retry for read-only tasks; a
  failing agent's tasks fail with a spoken notice instead of hanging; an optional `fallback` entry takes new tasks.
- **Data scoping.** An agent receives only what the orchestrator passes in its request; sensitive values captured
  through chapter 4's screen forms go only to the agent whose task asked for them.
- **Tracing.** Every top-level task gets a `trace_id` carried to its children and to every agent request (W3C
  `traceparent` header), and on the wire.
- **Caller continuity.** `tasks.resume_key` (chapter 3, not yet built) lets the tasks and their updates follow a caller
  who calls back or changes device.
- **The push text.** The signed post-call push carries the narrator's short update alongside the full result.

## Decisions made and what was given up

| Decision | Chosen | Given up |
|---|---|---|
| Who routes work to agents | a large orchestrator model on the LLM tier, agents as non-blocking tools | routing by the small front (cheaper, but it cannot plan multi-step or multi-agent work) |
| Limits | operator settings, none by default, queue instead of refuse | a hard safety cap in code; tenants must set quotas |
| Publishing results | new `task_output` at completion and `GET /v1/tasks/events` | changing `task_result` (would break current clients) |
| Speaking results | at a slot only, `ask` by default when the conversation is active | the earliest possible spoken update |
| Rewording | the front model, checked against the result, template fallback at 400 ms | the background's own words (longer, out of context) |
| Deadline questions | cut the agent at a clause boundary, never the caller | asking over the caller to meet a very short deadline |
| Follow-ups | same agent with its continuation handle, or the stored result | a fresh task each time (loses the agent's context) |

## What changes in the code

| Where | Change |
|---|---|
| `core/interaction/tasks.rs` | limits from config; `parent_id`, `agent`; finished tasks kept for follow-ups with their handle; continuation revision |
| `core/interaction/background.rs` | the orchestrator run: agent tools that start child tasks and return at once; child results fed back to its history |
| `core/interaction/remote.rs`, `bud_agent.rs` | many tasks per entry; continuation handles; `expires_in_ms`; per-entry breaker and timeout |
| `core/interaction/config.rs` | `tasks.orchestrator`, `tasks.agents`, `tasks.limits`, `tasks.announce`, `tasks.follow_up_ttl_s`; catalog `description`, `max_concurrent`, `question_deadline_ms`, `fallback` |
| `core/interaction/policy.rs` | the update inbox and slots; `ask`/`tell`/`screen`; deadline questions with resume; `follow_up` |
| `core/interaction/front_model.rs`, `rules.rs` | the narrator prompt and template; the `follow_up` act |
| `core/interaction/floor.rs` | resume of an interrupted answer from the first unheard sentence |
| `core/interaction/events.rs`, `handlers/tasks.rs`, `routes/api.rs` | `task_output`, `task_seen`, `parent_id`/`agent`/`trace_id`; `GET /v1/tasks/events` (OpenAPI) |
| SDKs | the new events and fields |

## Tests to write first

| Test | What it proves |
|---|---|
| `limits_come_from_config_and_excess_work_waits` | no refusal for count; queue order |
| `an_orchestrator_starts_two_agents_in_one_round_and_hears_both_results` | non-blocking dispatch and fan-in |
| `a_result_is_published_at_completion_while_the_agent_is_speaking` | `task_output` before any speech |
| `an_update_waits_for_the_end_of_the_answer_then_is_offered` | slots and `ask` |
| `two_updates_at_one_slot_become_one_offer` | coalescing |
| `a_spoken_update_never_contains_a_number_the_result_lacks` | narrator guard and fallback |
| `an_urgent_question_cuts_at_a_clause_and_the_answer_resumes` | deadline questions and resume |
| `an_urgent_question_never_starts_over_the_caller` | waits for the pause |
| `a_follow_up_question_is_answered_from_the_stored_result` | follow-up from the result |
| `a_follow_up_action_continues_the_same_agent_context` | continuation handle |
| `a_hesitant_request_revises_the_provisional_task` | one task for one request |
| `a_failing_agent_does_not_affect_another_agent` | per-agent breaker |

Live scenarios (the live script's stand-in A2A and webhook agents, two at once, and a local large model as the
orchestrator): **MA1** three tasks on two agents and a general question in one call, with every `task_output` arriving
before its spoken update and no update cutting an answer; **MA2** an urgent question with a 5 s deadline asked within
the deadline while the agent is mid-answer, then the answer resumed; **MA3** a follow-up question answered from the
result and a follow-up action continuing the same A2A `contextId`; **MA4** `GET /v1/tasks/events` streaming the
results to a second client. The experience evaluation adds a multi-task set: answer completeness (never cut by an
update), update latency after completion, the narrator's length and faithfulness, and deadline questions answered in
time.

## What ships in which release

All of it in an **orchestration track** after Release 2 (section 11 of the plan): it builds on durable tasks, the
remote backends and the front model, and needs no provider work.

## Risks and how each is handled

| Risk | Handling |
|---|---|
| Unlimited work runs up cost | tenant quotas and spend caps; queue, never silent drops; per-tenant metrics |
| The orchestrator adds delay before work starts | the front acknowledges at once; speculative start applies to the orchestrator run |
| A reworded update says something the result does not | the result is a source for the numbers-and-names check; template fallback |
| Too many spoken updates | `ask` by default, coalescing, progress only when the caller is waiting |
| An urgent question annoys the caller | only when the deadline cannot wait for a slot; never over the caller |
| Follow-up bound to the wrong task | default to the last spoken update; ask when two titles match |

## What is still open

- The narrator's quality on the small front model needs measuring (length, faithfulness, fit); a larger front profile
  may be needed for it.
- A2A has no standard deadline field for a question; the metadata name needs agreeing with agent authors.
- Nothing in this chapter depends on budprompt: Bud agents take part through A2A or MCP like any other agent.

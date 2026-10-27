//! Interaction mode's routes: `GET /v1/tasks` (the caller's tasks, from the process-wide task
//! store), the webhook callback, and what a tenant keeps across sessions (its personas, and
//! what is known about its callers).

use std::convert::Infallible;

use axum::extract::{Path, Query};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use futures::stream::Stream;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;

use crate::auth::Auth;
use crate::core::interaction::remote::{self, CallbackRefused};
use crate::core::interaction::store::{CallbackPlace, RelayedCallback};
use crate::core::interaction::{config, tasks, tenant};

/// The task store's owner key for a caller: the authenticated id, or the empty string when the
/// request carries none (authentication off). A session stores its tasks under the same key.
pub fn owner(auth: &Auth) -> &str {
    auth.id.as_deref().unwrap_or_default()
}

#[derive(Debug, Default, Deserialize)]
pub struct TasksQuery {
    /// Only the tasks of this session.
    pub stream_id: Option<String>,
}

/// Whether a caller may read the tasks of `session`: one with an authenticated id reads its
/// own sessions; one without reads only a session it names.
fn may_read(auth: &Auth, named: Option<&str>, session: &str) -> bool {
    named.map_or(auth.id.is_some(), |named| named == session)
}

/// `GET /v1/tasks`
///
/// Lists the tasks of the caller's sessions: the live ones, and finished ones for ten minutes.
/// With a task Redis these are the sessions of every gateway process that shares it.
/// A caller the gateway has no id for (authentication off, or an auth service that names
/// nobody) shares its owner key with every other such caller, so it gets only the session it
/// names with `stream_id`, and nothing without one.
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/v1/tasks",
        params(
            ("stream_id" = Option<String>, Query, description = "Only the tasks of this session; required when the request carries no authenticated id")
        ),
        responses(
            (status = 200, description = "`{\"tasks\": [...]}`: each task's id, ref, revision, title, state, cancelling flag, effects, open question, side-effects flag, start time, and its place in the task tree: `parent_id` (null for a caller's request), `agent` (the catalog entry that runs it, the server's tool for a call of an MCP server's tool, or null) and `trace_id`; a follow-up also has `follows` (the finished task it continues)", body = Object)
        ),
        security(
            ("bearer_auth" = [])
        ),
        tag = "tasks"
    )
)]
pub async fn list_tasks(
    Extension(auth): Extension<Auth>,
    Query(query): Query<TasksQuery>,
) -> Json<Value> {
    let session = query.stream_id.as_deref();
    let tasks = match (&auth.id, session) {
        (None, None) => Vec::new(),
        _ => tasks::store().list_shared(owner(&auth), session).await,
    };
    Json(json!({ "tasks": tasks }))
}

#[derive(Debug, Default, Deserialize)]
pub struct TaskEventsQuery {
    /// Only the events of this session.
    pub stream_id: Option<String>,
    /// Only the events of this task.
    pub task_id: Option<String>,
    /// Only the events of this task's children.
    pub parent_id: Option<String>,
}

impl TaskEventsQuery {
    /// Whether `auth`'s caller may read the event, and asked for it.
    fn wants(&self, auth: &Auth, event: &tasks::Published) -> bool {
        let is = |wanted: &Option<String>, value: Option<&str>| {
            wanted.as_deref().is_none_or(|wanted| Some(wanted) == value)
        };
        event.owner == owner(auth)
            && may_read(auth, self.stream_id.as_deref(), &event.session)
            && is(&self.task_id, Some(&event.task_id))
            && is(&self.parent_id, event.parent_id.as_deref())
    }
}

/// `GET /v1/tasks/events`
///
/// The caller's task events as server-sent events, from the moment of the request: each
/// `task_*` event of a session's socket (`task_created`, `task_progress`, `task_question`,
/// `task_answer_received`, `task_output`, `task_result`, `task_cancelled`) as one event named
/// after its `type`, with the session's `stream_id` added. Tasks that run on after their call
/// are included. Who may read is decided as for `GET /v1/tasks`. The events are those of the
/// tasks this gateway process runs.
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/v1/tasks/events",
        params(
            ("stream_id" = Option<String>, Query, description = "Only the events of this session; required when the request carries no authenticated id"),
            ("task_id" = Option<String>, Query, description = "Only the events of this task"),
            ("parent_id" = Option<String>, Query, description = "Only the events of this task's children")
        ),
        responses(
            (status = 200, description = "A `text/event-stream`: one event per task event, named after its `type` (`task_output` carries the whole result: `text`, `data`, `artifacts`, `effects`), with `stream_id`; `lagged` with the number of events missed when the reader fell behind", content_type = "text/event-stream", body = String)
        ),
        security(
            ("bearer_auth" = [])
        ),
        tag = "tasks"
    )
)]
pub async fn task_events(
    Extension(auth): Extension<Auth>,
    Query(query): Query<TaskEventsQuery>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let events = tasks::store().subscribe();
    let stream = futures::stream::unfold(
        (events, auth, query),
        |(mut events, auth, query)| async move {
            loop {
                let sse = match events.recv().await {
                    Ok(event) if query.wants(&auth, &event) => Event::default()
                        .event(event.kind.as_str())
                        .json_data(&event.event)
                        .unwrap_or_default(),
                    Ok(_) => continue,
                    // A reader that fell behind is told how many it missed, and reads on.
                    Err(RecvError::Lagged(missed)) => {
                        Event::default().event("lagged").data(missed.to_string())
                    }
                    Err(RecvError::Closed) => return None,
                };
                return Some((Ok(sse), (events, auth, query)));
            }
        },
    );
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// `GET /v1/tasks/{task_id}`
///
/// One task of the caller's, with the session it belongs to and, once it has ended, how:
/// `result` holds its `state`, its `summary` (the answer of a completed task, else what went
/// wrong) and its `effects`. The rule of `GET /v1/tasks` decides who may read it; a task the
/// caller may not read is answered like one that does not exist.
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/v1/tasks/{task_id}",
        params(
            ("task_id" = String, Path, description = "The task's id, as in `task_created`"),
            ("stream_id" = Option<String>, Query, description = "The task's session; required when the request carries no authenticated id")
        ),
        responses(
            (status = 200, description = "The task as in `GET /v1/tasks`, with `stream_id` and `result` (`null` while it runs; else `state`, `summary` and `effects`)", body = Object),
            (status = 404, description = "No such task, or not the caller's")
        ),
        security(
            ("bearer_auth" = [])
        ),
        tag = "tasks"
    )
)]
pub async fn get_task(
    Extension(auth): Extension<Auth>,
    Path(task_id): Path<String>,
    Query(query): Query<TasksQuery>,
) -> Result<Json<Value>, StatusCode> {
    let stored = tasks::store().get(owner(&auth), &task_id).await;
    let readable =
        |stored: &tasks::Stored| may_read(&auth, query.stream_id.as_deref(), &stored.session);
    let stored = stored.filter(readable).ok_or(StatusCode::NOT_FOUND)?;
    let mut task = serde_json::to_value(&stored.view).unwrap_or_default();
    task["stream_id"] = json!(stored.session);
    task["result"] = match &stored.result {
        Some(result) => json!({
            "state": result.state,
            "summary": result.summary,
            "effects": result.effects,
        }),
        None => Value::Null,
    };
    Ok(Json(task))
}

/// `POST /v1/tasks/{task_id}/callback`
///
/// A webhook backend reports on a task it was given: the body is one event or a list of
/// events (`progress`, `effect`, `question`, `result`, `failed`, `cancelled`, `control`).
/// The request is signed like the gateway's own message to the webhook: `X-WaaV-Timestamp`
/// and `X-WaaV-Signature`, the HMAC-SHA256 of `"<timestamp>.<body>"` under the backend's
/// secret. An A2A agent's push notification for a task (a task, a message, or a status or
/// artifact update) comes to the same route with the token of the task's push configuration
/// in `X-A2A-Notification-Token`. The process that runs the task takes it.
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        post,
        path = "/v1/tasks/{task_id}/callback",
        params(
            ("task_id" = String, Path, description = "The `task_id` the webhook was given"),
            ("X-WaaV-Timestamp" = String, Header, description = "Unix seconds, within five minutes of the gateway's clock"),
            ("X-WaaV-Signature" = String, Header, description = "`sha256=` and the hex HMAC-SHA256 of `<timestamp>.<body>` under the backend's secret"),
            ("X-A2A-Notification-Token" = Option<String>, Header, description = "An A2A agent's push: the token the gateway sent in the task's `pushNotificationConfig` (in place of the two headers above)")
        ),
        request_body(content = Object, description = "One event or a list of events"),
        responses(
            (status = 202, description = "The events were handed to the task's run, or (with a task Redis) to the gateway process that runs it, which checks them as its own"),
            (status = 400, description = "The body is not an event or a list of events"),
            (status = 401, description = "The signature (or a push's token) is missing, wrong or too old, or the backend has no secret"),
            (status = 404, description = "No run of this process waits for that task, and no other process runs it"),
            (status = 409, description = "The task was revised since: the callback is of a replaced run"),
            (status = 503, description = "Another gateway process runs the task but the task Redis did not take the callback: try again (Retry-After)")
        ),
        tag = "tasks"
    )
)]
pub async fn task_callback(
    Path(task_id): Path<String>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
    let sender = remote::CallbackFrom {
        timestamp: header("x-waav-timestamp"),
        signature: header("x-waav-signature"),
        token: header("x-a2a-notification-token"),
        token_sha256: None,
    };
    let status = match remote::callback(&task_id, sender, &body) {
        Ok(()) => StatusCode::ACCEPTED,
        // One with no proof at all goes no further: no other process could take it.
        Err(CallbackRefused::UnknownTask) if !sender.has_proof() => StatusCode::UNAUTHORIZED,
        // Another gateway process may run it (the task Redis says which).
        Err(CallbackRefused::UnknownTask) => {
            let relayed = RelayedCallback::of(task_id, sender, body);
            match tasks::store().relay_callback(relayed).await {
                CallbackPlace::Relayed => StatusCode::ACCEPTED,
                CallbackPlace::Unknown => StatusCode::NOT_FOUND,
                CallbackPlace::NotRelayed => {
                    let retry = [(axum::http::header::RETRY_AFTER, "2")];
                    return (StatusCode::SERVICE_UNAVAILABLE, retry).into_response();
                }
            }
        }
        Err(CallbackRefused::Unsigned) => StatusCode::UNAUTHORIZED,
        Err(CallbackRefused::Stale) => StatusCode::CONFLICT,
        Err(CallbackRefused::Invalid) => StatusCode::BAD_REQUEST,
    };
    status.into_response()
}

/// `PUT /v1/interaction/personas/{id}`
///
/// Registers a new version of one of the tenant's personas: the persona object of
/// `persona.schema.json`, checked like the packaged personas. A session of the tenant selects
/// it with `front.persona` (`id`, or `id@version`). Needs the server's
/// `interaction.tenant_personas`.
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        put,
        path = "/v1/interaction/personas/{id}",
        params(
            ("id" = String, Path, description = "The persona's id: 1 to 64 letters, digits, `_`, `.` or `-`")
        ),
        request_body(content = Object, description = "The persona (`persona.schema.json`); its `id` is the path's"),
        responses(
            (status = 200, description = "`{\"id\", \"version\"}`: the version registered (the newest five are kept)"),
            (status = 404, description = "This server does not take tenant personas"),
            (status = 409, description = "The tenant keeps 20 personas already"),
            (status = 422, description = "`{\"error\"}`: what the persona fails")
        ),
        security(
            ("bearer_auth" = [])
        ),
        tag = "tasks"
    )
)]
pub async fn put_persona(
    Extension(auth): Extension<Auth>,
    Path(id): Path<String>,
    Json(spec): Json<Value>,
) -> (StatusCode, Json<Value>) {
    if !config::server().tenant_personas {
        return not_found("this server does not take tenant personas");
    }
    match tenant::put_persona(owner(&auth), &id, spec).await {
        Ok(version) => (
            StatusCode::OK,
            Json(json!({ "id": id, "version": version })),
        ),
        Err(tenant::PutRefused::TooMany) => {
            let error = format!(
                "a tenant keeps at most {} personas",
                tenant::PERSONAS_PER_TENANT
            );
            refused(StatusCode::CONFLICT, error)
        }
        Err(tenant::PutRefused::Invalid(error)) => refused(StatusCode::UNPROCESSABLE_ENTITY, error),
    }
}

/// `GET /v1/interaction/personas/{id}`
///
/// One of the tenant's personas: the newest version, or the one `id@version` names, with the
/// numbers of the versions kept. Needs the server's `interaction.tenant_personas`.
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/v1/interaction/personas/{id}",
        params(
            ("id" = String, Path, description = "The persona's id, or `id@version` for a version the tenant still keeps")
        ),
        responses(
            (status = 200, description = "`{\"id\", \"version\", \"versions\", \"persona\"}`: the version shown, every version kept (oldest first) and the persona object of that version", body = Object),
            (status = 404, description = "This server does not take tenant personas, or the tenant has no such persona or version")
        ),
        security(
            ("bearer_auth" = [])
        ),
        tag = "tasks"
    )
)]
pub async fn get_persona(
    Extension(auth): Extension<Auth>,
    Path(id): Path<String>,
) -> (StatusCode, Json<Value>) {
    if !config::server().tenant_personas {
        return not_found("this server does not take tenant personas");
    }
    let (name, wanted) = match id.split_once('@') {
        Some((name, version)) => (name, version.parse::<u32>().ok().or(Some(0))),
        None => (id.as_str(), None),
    };
    let versions = tenant::persona_versions(owner(&auth), name)
        .await
        .unwrap_or_default();
    let shown = match wanted {
        Some(version) => versions.iter().find(|(kept, _)| *kept == version),
        None => versions.last(),
    };
    let Some((version, persona)) = shown else {
        return not_found("the tenant has no such persona");
    };
    let kept: Vec<u32> = versions.iter().map(|(version, _)| *version).collect();
    let body = json!({ "id": name, "version": version, "versions": kept, "persona": persona });
    (StatusCode::OK, Json(body))
}

/// `DELETE /v1/interaction/personas/{id}`
///
/// Removes every version of one of the tenant's personas. A session that runs it keeps it to
/// its end; a session that selects it later gets the default persona. Needs the server's
/// `interaction.tenant_personas`.
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        delete,
        path = "/v1/interaction/personas/{id}",
        params(
            ("id" = String, Path, description = "The persona's id")
        ),
        responses(
            (status = 204, description = "Removed"),
            (status = 404, description = "This server does not take tenant personas, or the tenant has no such persona"),
            (status = 503, description = "`{\"error\"}`: the task Redis did not take it")
        ),
        security(
            ("bearer_auth" = [])
        ),
        tag = "tasks"
    )
)]
pub async fn delete_persona(
    Extension(auth): Extension<Auth>,
    Path(id): Path<String>,
) -> (StatusCode, Json<Value>) {
    if !config::server().tenant_personas {
        return not_found("this server does not take tenant personas");
    }
    match tenant::delete_persona(owner(&auth), &id).await {
        Ok(true) => (StatusCode::NO_CONTENT, Json(Value::Null)),
        Ok(false) => not_found("the tenant has no such persona"),
        Err(error) => refused(StatusCode::SERVICE_UNAVAILABLE, error),
    }
}

fn not_found(error: &str) -> (StatusCode, Json<Value>) {
    refused(StatusCode::NOT_FOUND, error)
}

fn refused(status: StatusCode, error: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": error.to_string() })))
}

/// The body of `PUT /v1/interaction/memory/{subject}`.
#[derive(Debug, Deserialize)]
pub struct MemoryBody {
    pub facts: Vec<String>,
}

/// `PUT /v1/interaction/memory/{subject}`
///
/// What is known about one of the tenant's callers, for later calls: at most ten facts of at
/// most 80 characters, with no number, code, address, amount or link, the most important
/// first. It replaces what was kept, an empty list forgets the caller, and it is kept for 90
/// days. A session reads the first three when it carries an identity token the operator signed
/// for that caller (`front.caller.identity_token`); after such a call the background may add
/// facts in front. Needs the server's `interaction.caller_memory_key`.
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        put,
        path = "/v1/interaction/memory/{subject}",
        params(
            ("subject" = String, Path, description = "The caller, as the `sub` of the identity token")
        ),
        request_body(content = Object, description = "`{\"facts\": [...]}`"),
        responses(
            (status = 204, description = "Kept"),
            (status = 404, description = "This server keeps no caller memory"),
            (status = 422, description = "`{\"error\"}`: a fact that may not be kept"),
            (status = 503, description = "`{\"error\"}`: the task Redis did not take it")
        ),
        security(
            ("bearer_auth" = [])
        ),
        tag = "tasks"
    )
)]
pub async fn put_memory(
    Extension(auth): Extension<Auth>,
    Path(subject): Path<String>,
    Json(body): Json<MemoryBody>,
) -> (StatusCode, Json<Value>) {
    let Some(key) = config::server().caller_memory_key.clone() else {
        return not_found("this server keeps no caller memory");
    };
    if let Err(error) = tenant::check_facts(&body.facts) {
        return refused(StatusCode::UNPROCESSABLE_ENTITY, error);
    }
    match tenant::remember(&key, owner(&auth), &subject, body.facts).await {
        Ok(()) => (StatusCode::NO_CONTENT, Json(Value::Null)),
        Err(error) => refused(StatusCode::SERVICE_UNAVAILABLE, error),
    }
}

/// `GET /v1/interaction/memory/{subject}`
///
/// Everything kept about one of the tenant's callers, newest first: what `PUT` wrote and what
/// the background added after calls. Needs the server's `interaction.caller_memory_key`.
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/v1/interaction/memory/{subject}",
        params(
            ("subject" = String, Path, description = "The caller, as the `sub` of the identity token")
        ),
        responses(
            (status = 200, description = "`{\"facts\": [...]}`: empty when nothing is kept", body = Object),
            (status = 404, description = "This server keeps no caller memory"),
            (status = 503, description = "`{\"error\"}`: the task Redis did not answer")
        ),
        security(
            ("bearer_auth" = [])
        ),
        tag = "tasks"
    )
)]
pub async fn get_memory(
    Extension(auth): Extension<Auth>,
    Path(subject): Path<String>,
) -> (StatusCode, Json<Value>) {
    let Some(key) = config::server().caller_memory_key.clone() else {
        return not_found("this server keeps no caller memory");
    };
    match tenant::all_facts(&key, owner(&auth), &subject).await {
        Ok(facts) => (StatusCode::OK, Json(json!({ "facts": facts }))),
        Err(error) => refused(StatusCode::SERVICE_UNAVAILABLE, error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::interaction::types::{Effects, Lineage, TaskId, TaskRef, TaskState, TaskView};

    #[tokio::test]
    async fn a_caller_sees_only_its_own_tasks() {
        let view = TaskView {
            id: TaskId("task_handler_test".to_owned()),
            task_ref: TaskRef("t1".to_owned()),
            revision: 1,
            title: "your table booking".to_owned(),
            state: TaskState::Working,
            cancelling: false,
            effects: Effects::NoneCommitted,
            open_question: None,
            side_effects: true,
            started_at: 4_180,
            lineage: Lineage {
                follows: None,
                parent_id: Some(TaskId("task_parent".to_owned())),
                agent: Some("browser".to_owned()),
                trace_id: "4bf92f3577b34da6a3ce929d0e0e4736".to_owned(),
            },
        };
        tasks::store().upsert("tasks-handler-test-owner", "session-1", view);

        let all = || Query(TasksQuery::default());
        let Json(mine) = list_tasks(Extension(Auth::new("tasks-handler-test-owner")), all()).await;
        assert_eq!(
            mine,
            json!({ "tasks": [{
                "id": "task_handler_test",
                "task_ref": "t1",
                "revision": 1,
                "title": "your table booking",
                "state": "working",
                "cancelling": false,
                "effects": "none_committed",
                "open_question": null,
                "side_effects": true,
                "started_at": 4180,
                "parent_id": "task_parent",
                "agent": "browser",
                "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736",
            }] })
        );
        let Json(other) = list_tasks(Extension(Auth::new("tasks-handler-test-other")), all()).await;
        assert_eq!(other, json!({ "tasks": [] }));
    }

    #[tokio::test]
    async fn a_caller_without_an_id_sees_only_the_session_it_names() {
        let view = TaskView {
            id: TaskId("task_anonymous_test".to_owned()),
            task_ref: TaskRef("t1".to_owned()),
            revision: 1,
            title: "a refund".to_owned(),
            state: TaskState::Working,
            cancelling: false,
            effects: Effects::NoneCommitted,
            open_question: None,
            side_effects: true,
            started_at: 0,
            lineage: Default::default(),
        };
        // Every caller without an id stores under the same owner key.
        tasks::store().upsert("", "tasks-handler-anonymous-session", view);
        let ids = |Json(body): Json<Value>| -> Vec<Value> {
            let tasks = body["tasks"].as_array().expect("a list");
            tasks.iter().map(|task| task["id"].clone()).collect()
        };
        let named = |stream_id: &str| {
            Query(TasksQuery {
                stream_id: Some(stream_id.to_owned()),
            })
        };

        let nobody = || Extension(Auth::empty());
        assert!(ids(list_tasks(nobody(), Query(TasksQuery::default())).await).is_empty());
        assert!(ids(list_tasks(nobody(), named("another-session")).await).is_empty());
        assert_eq!(
            ids(list_tasks(nobody(), named("tasks-handler-anonymous-session")).await),
            [json!("task_anonymous_test")]
        );

        // The same rule for one task: only with the session it belongs to.
        let one = |stream_id: Option<&str>| {
            let query = Query(TasksQuery {
                stream_id: stream_id.map(str::to_owned),
            });
            get_task(nobody(), Path("task_anonymous_test".to_owned()), query)
        };
        assert_eq!(one(None).await.unwrap_err(), StatusCode::NOT_FOUND);
        assert_eq!(
            one(Some("another-session")).await.unwrap_err(),
            StatusCode::NOT_FOUND
        );
        let Json(task) = one(Some("tasks-handler-anonymous-session")).await.unwrap();
        assert_eq!(task["id"], "task_anonymous_test");
        assert_eq!(task["result"], Value::Null);
    }

    #[tokio::test]
    async fn one_task_is_read_with_its_result_by_its_owner_only() {
        use crate::core::interaction::types::Outcome;
        let session = tasks::Session {
            owner: "tasks-handler-result-owner".to_owned(),
            id: "session-9".to_owned(),
            push_url: Some("https://hooks.example.com/tasks".to_owned()),
            grace: std::time::Duration::from_secs(60),
            resume: None,
        };
        let view = TaskView {
            id: TaskId("task_result_test".to_owned()),
            task_ref: TaskRef("t2".to_owned()),
            revision: 1,
            title: "a table for two".to_owned(),
            state: TaskState::Completed,
            cancelling: false,
            effects: Effects::Committed(vec!["table booking".to_owned()]),
            open_question: None,
            side_effects: true,
            started_at: 900,
            lineage: Default::default(),
        };
        tasks::store().write(&session, view.clone());
        let outcome = Outcome {
            update: None,
            task_id: view.id.clone(),
            task_ref: view.task_ref.clone(),
            revision: 1,
            seq: 6,
            state: TaskState::Completed,
            summary: "Your table for two is booked.".to_owned(),
            effects: view.effects.clone(),
            lineage: Default::default(),
        };
        tasks::store().result(&session, &outcome);

        let one = |who: &str, stream_id: Option<&str>| {
            let query = Query(TasksQuery {
                stream_id: stream_id.map(str::to_owned),
            });
            get_task(
                Extension(Auth::new(who)),
                Path("task_result_test".to_owned()),
                query,
            )
        };
        let Json(task) = one("tasks-handler-result-owner", None).await.unwrap();
        assert_eq!(
            task,
            json!({
                "id": "task_result_test",
                "task_ref": "t2",
                "revision": 1,
                "title": "a table for two",
                "state": "completed",
                "cancelling": false,
                "effects": ["table booking"],
                "open_question": null,
                "side_effects": true,
                "started_at": 900,
                "parent_id": null,
                "agent": null,
                "trace_id": "",
                "stream_id": "session-9",
                "result": {
                    "state": "completed",
                    "summary": "Your table for two is booked.",
                    "effects": ["table booking"],
                },
            })
        );
        // The operator's push address is nobody's to read.
        assert!(!task.to_string().contains("hooks.example.com"));
        assert!(
            one("tasks-handler-result-owner", Some("session-9"))
                .await
                .is_ok()
        );
        let wrong = one("tasks-handler-result-owner", Some("session-8")).await;
        assert_eq!(wrong.unwrap_err(), StatusCode::NOT_FOUND);
        let other = one("tasks-handler-result-other", None).await;
        assert_eq!(other.unwrap_err(), StatusCode::NOT_FOUND);
        let missing = get_task(
            Extension(Auth::new("tasks-handler-result-owner")),
            Path("task_missing".to_owned()),
            Query(TasksQuery::default()),
        );
        assert_eq!(missing.await.unwrap_err(), StatusCode::NOT_FOUND);
    }
    #[tokio::test]
    async fn the_task_events_stream_carries_the_callers_events_only() {
        use axum::response::IntoResponse;
        use http_body_util::BodyExt;

        let owner = "tasks-events-test-owner";
        let session = |id: &str| tasks::Session {
            owner: owner.to_owned(),
            id: id.to_owned(),
            push_url: None,
            grace: std::time::Duration::ZERO,
            resume: None,
        };
        let query = TaskEventsQuery {
            stream_id: Some("call-1".to_owned()),
            parent_id: Some("task_parent".to_owned()),
            ..TaskEventsQuery::default()
        };
        let stream = task_events(Extension(Auth::new(owner)), Query(query)).await;
        let mut body = stream.into_response().into_body();
        let event = |task: &str, parent: &str| json!({ "type": "task_output", "task_id": task, "parent_id": parent, "text": "Done." });
        // Another session, another task tree, another caller: none of them.
        tasks::store().publish(&session("call-2"), event("task_a", "task_parent"));
        tasks::store().publish(&session("call-1"), event("task_b", "task_other"));
        let stranger = tasks::Session {
            owner: "someone-else".to_owned(),
            ..session("call-1")
        };
        tasks::store().publish(&stranger, event("task_c", "task_parent"));
        tasks::store().publish(&session("call-1"), event("task_d", "task_parent"));
        let frame = tokio::time::timeout(std::time::Duration::from_secs(5), body.frame())
            .await
            .expect("an event in time")
            .expect("a frame")
            .expect("no error");
        let text = String::from_utf8(frame.into_data().expect("data").to_vec()).unwrap();
        assert!(text.starts_with("event: task_output\ndata: "), "{text}");
        let data: Value =
            serde_json::from_str(text.lines().nth(1).unwrap().trim_start_matches("data: "))
                .unwrap();
        assert_eq!(
            (data["task_id"].as_str(), data["stream_id"].as_str()),
            (Some("task_d"), Some("call-1"))
        );

        // Without an authenticated id the session must be named, as for `GET /v1/tasks`.
        let anonymous = Auth::default();
        let named = TaskEventsQuery {
            stream_id: Some("call-1".to_owned()),
            ..TaskEventsQuery::default()
        };
        let published = |session: &str| tasks::Published {
            owner: String::new(),
            session: session.to_owned(),
            task_id: "task_e".to_owned(),
            parent_id: None,
            kind: "task_created".to_owned(),
            event: Value::Null,
        };
        assert!(named.wants(&anonymous, &published("call-1")));
        assert!(!named.wants(&anonymous, &published("call-2")));
        assert!(!TaskEventsQuery::default().wants(&anonymous, &published("call-1")));
    }
}

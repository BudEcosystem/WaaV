//! A2A 1.0's gRPC binding (`lf.a2a.v1.A2AService` of the specification's `a2a.proto`): the
//! few messages the gateway sends and reads, written by hand (no build step, no new crate:
//! `tonic` and `prost` are already the gateway's). A request is made from the JSON the
//! JSON-RPC binding sends, and a reply is written back as that binding's JSON (ProtoJSON), so
//! one reader in [`super::remote`] serves the three bindings. Fields the gateway does not read
//! are not declared: prost skips them.
//!
//! `SendMessage`, `GetTask`, `CancelTask`, `SendStreamingMessage` (a task's events as they
//! come, [`stream`]) and `GetExtendedAgentCard` (the card fields the gateway reads) are served.
//! Every connection the channel makes, the first and each one after the agent's name is
//! resolved again, goes only to an address the SSRF rules allow ([`checked_connect`]).

use std::collections::HashMap;
use std::sync::{LazyLock, PoisonError};
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::{Map, Value, json};
use tonic::codegen::http::uri::PathAndQuery;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};

use super::background::NotPosted;
use crate::core::net::{HTTP_URL_SCHEMES, validate_url_for_ssrf};

const SERVICE: &str = "/lf.a2a.v1.A2AService/";
/// How long one call may take, as an HTTP call of a backend.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// `TaskState`, by number.
const STATES: [&str; 9] = [
    "TASK_STATE_UNSPECIFIED",
    "TASK_STATE_SUBMITTED",
    "TASK_STATE_WORKING",
    "TASK_STATE_COMPLETED",
    "TASK_STATE_FAILED",
    "TASK_STATE_CANCELED",
    "TASK_STATE_INPUT_REQUIRED",
    "TASK_STATE_REJECTED",
    "TASK_STATE_AUTH_REQUIRED",
];
/// `Role`, by number.
const ROLES: [&str; 3] = ["ROLE_UNSPECIFIED", "ROLE_USER", "ROLE_AGENT"];

#[derive(Clone, PartialEq, prost::Message)]
pub struct Part {
    #[prost(oneof = "part::Content", tags = "1, 2, 3, 4")]
    pub content: Option<part::Content>,
    #[prost(string, tag = "6")]
    pub filename: String,
    #[prost(string, tag = "7")]
    pub media_type: String,
}

pub mod part {
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Content {
        #[prost(string, tag = "1")]
        Text(String),
        #[prost(bytes, tag = "2")]
        Raw(Vec<u8>),
        #[prost(string, tag = "3")]
        Url(String),
        #[prost(message, tag = "4")]
        Data(prost_types::Value),
    }
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Message {
    #[prost(string, tag = "1")]
    pub message_id: String,
    #[prost(string, tag = "2")]
    pub context_id: String,
    #[prost(string, tag = "3")]
    pub task_id: String,
    #[prost(int32, tag = "4")]
    pub role: i32,
    #[prost(message, repeated, tag = "5")]
    pub parts: Vec<Part>,
    #[prost(message, optional, tag = "6")]
    pub metadata: Option<prost_types::Struct>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct TaskStatus {
    #[prost(int32, tag = "1")]
    pub state: i32,
    #[prost(message, optional, tag = "2")]
    pub message: Option<Message>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Artifact {
    #[prost(string, tag = "1")]
    pub artifact_id: String,
    #[prost(string, tag = "2")]
    pub name: String,
    #[prost(message, repeated, tag = "4")]
    pub parts: Vec<Part>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Task {
    #[prost(string, tag = "1")]
    pub id: String,
    #[prost(string, tag = "2")]
    pub context_id: String,
    #[prost(message, optional, tag = "3")]
    pub status: Option<TaskStatus>,
    #[prost(message, repeated, tag = "4")]
    pub artifacts: Vec<Artifact>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct PushConfig {
    #[prost(string, tag = "4")]
    pub url: String,
    #[prost(string, tag = "5")]
    pub token: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SendMessageConfiguration {
    #[prost(string, repeated, tag = "1")]
    pub accepted_output_modes: Vec<String>,
    #[prost(message, optional, tag = "2")]
    pub task_push_notification_config: Option<PushConfig>,
    #[prost(bool, tag = "4")]
    pub return_immediately: bool,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SendMessageRequest {
    #[prost(message, optional, tag = "2")]
    pub message: Option<Message>,
    #[prost(message, optional, tag = "3")]
    pub configuration: Option<SendMessageConfiguration>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SendMessageResponse {
    #[prost(oneof = "send_message_response::Payload", tags = "1, 2")]
    pub payload: Option<send_message_response::Payload>,
}

pub mod send_message_response {
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Payload {
        #[prost(message, tag = "1")]
        Task(super::Task),
        #[prost(message, tag = "2")]
        Message(super::Message),
    }
}

/// `GetTaskRequest` and `CancelTaskRequest`: the task's id (field 2).
#[derive(Clone, PartialEq, prost::Message)]
pub struct TaskRequest {
    #[prost(string, tag = "2")]
    pub id: String,
}

/// `StreamResponse`: one event of `SendStreamingMessage`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct StreamResponse {
    #[prost(oneof = "stream_response::Payload", tags = "1, 2, 3, 4")]
    pub payload: Option<stream_response::Payload>,
}

pub mod stream_response {
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Payload {
        #[prost(message, tag = "1")]
        Task(super::Task),
        #[prost(message, tag = "2")]
        Message(super::Message),
        #[prost(message, tag = "3")]
        StatusUpdate(super::TaskStatusUpdateEvent),
        #[prost(message, tag = "4")]
        ArtifactUpdate(super::TaskArtifactUpdateEvent),
    }
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct TaskStatusUpdateEvent {
    #[prost(string, tag = "1")]
    pub task_id: String,
    #[prost(string, tag = "2")]
    pub context_id: String,
    #[prost(message, optional, tag = "3")]
    pub status: Option<TaskStatus>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct TaskArtifactUpdateEvent {
    #[prost(string, tag = "1")]
    pub task_id: String,
    #[prost(string, tag = "2")]
    pub context_id: String,
    #[prost(message, optional, tag = "3")]
    pub artifact: Option<Artifact>,
    #[prost(bool, tag = "4")]
    pub append: bool,
    #[prost(bool, tag = "5")]
    pub last_chunk: bool,
}

/// `GetExtendedAgentCardRequest` (its `tenant` is not used).
#[derive(Clone, PartialEq, prost::Message)]
pub struct CardRequest {}

/// `AgentCard`: the fields the gateway reads (its bindings, capabilities, security schemes,
/// description and skills).
#[derive(Clone, PartialEq, prost::Message)]
pub struct AgentCard {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(string, tag = "2")]
    pub description: String,
    #[prost(message, repeated, tag = "3")]
    pub supported_interfaces: Vec<AgentInterface>,
    #[prost(string, tag = "5")]
    pub version: String,
    #[prost(message, optional, tag = "7")]
    pub capabilities: Option<AgentCapabilities>,
    #[prost(map = "string, message", tag = "8")]
    pub security_schemes: HashMap<String, SecurityScheme>,
    #[prost(message, repeated, tag = "12")]
    pub skills: Vec<AgentSkill>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct AgentInterface {
    #[prost(string, tag = "1")]
    pub url: String,
    #[prost(string, tag = "2")]
    pub protocol_binding: String,
    #[prost(string, tag = "4")]
    pub protocol_version: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct AgentCapabilities {
    #[prost(bool, optional, tag = "1")]
    pub streaming: Option<bool>,
    #[prost(bool, optional, tag = "2")]
    pub push_notifications: Option<bool>,
    #[prost(bool, optional, tag = "4")]
    pub extended_agent_card: Option<bool>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct AgentSkill {
    #[prost(string, tag = "1")]
    pub id: String,
    #[prost(string, tag = "2")]
    pub name: String,
    #[prost(string, tag = "3")]
    pub description: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SecurityScheme {
    #[prost(oneof = "security_scheme::Scheme", tags = "1, 2, 3")]
    pub scheme: Option<security_scheme::Scheme>,
}

pub mod security_scheme {
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Scheme {
        #[prost(message, tag = "1")]
        ApiKey(super::ApiKeyScheme),
        #[prost(message, tag = "2")]
        Http(super::HttpAuthScheme),
        #[prost(message, tag = "3")]
        Oauth2(super::OAuth2Scheme),
    }
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ApiKeyScheme {
    #[prost(string, tag = "2")]
    pub location: String,
    #[prost(string, tag = "3")]
    pub name: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct HttpAuthScheme {
    #[prost(string, tag = "2")]
    pub scheme: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OAuth2Scheme {
    #[prost(message, optional, tag = "2")]
    pub flows: Option<OAuthFlows>,
}

/// `OAuthFlows`: the client-credentials flow (field 2) is the one the gateway uses.
#[derive(Clone, PartialEq, prost::Message)]
pub struct OAuthFlows {
    #[prost(message, optional, tag = "2")]
    pub client_credentials: Option<ClientCredentialsFlow>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ClientCredentialsFlow {
    #[prost(string, tag = "1")]
    pub token_url: String,
}

/// One A2A method on the gRPC binding at `endpoint`: `params` as the JSON-RPC binding sends
/// them, the reply as its `result`.
pub(super) async fn call(
    endpoint: &str,
    method: &str,
    params: &Value,
    headers: &[(&str, String)],
) -> Result<Value, NotPosted> {
    let channel = channel(endpoint).await?;
    match method {
        "SendMessage" => {
            let request = send_request(params);
            let reply: SendMessageResponse = unary(channel, method, request, headers).await?;
            Ok(match reply.payload {
                Some(send_message_response::Payload::Task(task)) => {
                    json!({ "task": task_json(&task) })
                }
                Some(send_message_response::Payload::Message(message)) => {
                    json!({ "message": message_json(&message) })
                }
                None => Value::Null,
            })
        }
        "GetTask" | "CancelTask" => {
            let id = params["id"].as_str().unwrap_or_default().to_owned();
            let task: Task = unary(channel, method, TaskRequest { id }, headers).await?;
            Ok(task_json(&task))
        }
        "GetExtendedAgentCard" => {
            let card: AgentCard = unary(channel, method, CardRequest {}, headers).await?;
            Ok(card_json(&card))
        }
        _ => Err(NotPosted::Unsent(format!(
            "{method} is not served on the gRPC binding"
        ))),
    }
}

/// A `SendStreamingMessage` call's events, each written as the JSON-RPC binding's `result`
/// of one event (`task`, `message`, `statusUpdate`, `artifactUpdate`). In a mutex only so the
/// reader that holds it is `Sync` (it is read through `&mut`, never locked).
pub(super) struct Stream(std::sync::Mutex<tonic::Streaming<StreamResponse>>);

impl Stream {
    /// The next event; `None` once the agent ended the stream.
    pub(super) async fn next(&mut self) -> Result<Option<Value>, NotPosted> {
        let events = self.0.get_mut().unwrap_or_else(PoisonError::into_inner);
        match events.message().await {
            Ok(Some(event)) => Ok(Some(stream_json(&event))),
            Ok(None) => Ok(None),
            Err(status) => Err(refused(&status)),
        }
    }
}

/// `SendStreamingMessage` on the gRPC binding at `endpoint`: `params` as the JSON-RPC binding
/// sends them.
pub(super) async fn stream(
    endpoint: &str,
    params: &Value,
    headers: &[(&str, String)],
) -> Result<Stream, NotPosted> {
    let channel = channel(endpoint).await?;
    let (mut grpc, request, path) = prepared(
        channel,
        "SendStreamingMessage",
        send_request(params),
        headers,
    )
    .await?;
    let codec = tonic::codec::ProstCodec::<SendMessageRequest, StreamResponse>::default();
    match grpc.server_streaming(request, path, codec).await {
        Ok(reply) => Ok(Stream(std::sync::Mutex::new(reply.into_inner()))),
        Err(status) => Err(refused(&status)),
    }
}

/// The agent's channel, made once per address (connected when first used). The address is
/// checked as every backend address is.
async fn channel(endpoint: &str) -> Result<Channel, NotPosted> {
    static CHANNELS: LazyLock<Mutex<HashMap<String, Channel>>> = LazyLock::new(Mutex::default);
    if let Some(channel) = CHANNELS.lock().get(endpoint) {
        return Ok(channel.clone());
    }
    let target = endpoint.to_owned();
    let checked =
        tokio::task::spawn_blocking(move || validate_url_for_ssrf(&target, HTTP_URL_SCHEMES));
    match checked.await {
        Ok(Ok(())) => {}
        Ok(Err(reason)) => return Err(NotPosted::Unsent(format!("URL refused: {reason}"))),
        Err(error) => return Err(NotPosted::Unsent(error.to_string())),
    }
    let unsent = |error: tonic::transport::Error| NotPosted::Unsent(error.to_string());
    let mut made = Endpoint::from_shared(endpoint.to_owned())
        .map_err(unsent)?
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(CALL_TIMEOUT);
    if endpoint.starts_with("https://") {
        made = made.tls_config(ClientTlsConfig::new()).map_err(unsent)?;
    }
    let channel = made.connect_with_connector_lazy(tower::service_fn(checked_connect));
    CHANNELS.lock().insert(endpoint.to_owned(), channel.clone());
    Ok(channel)
}

/// One connection of an agent's channel: the agent's name is resolved now (the channel
/// connects again after the agent went away, and the name may say another address by then),
/// and only an address the SSRF rules allow is dialled. The check at the channel's creation
/// does not stand for a later resolution.
async fn checked_connect(uri: tonic::transport::Uri) -> std::io::Result<tokio::net::TcpStream> {
    use std::io::{Error, ErrorKind};
    let host = uri.host().unwrap_or_default().trim_matches(['[', ']']);
    let tls = uri.scheme_str() == Some("https");
    let port = uri.port_u16().unwrap_or(if tls { 443 } else { 80 });
    let loopback = crate::core::net::loopback_endpoints_allowed();
    let mut refused = None;
    for address in tokio::net::lookup_host((host, port)).await? {
        if !loopback && crate::utils::url_validation::is_private_ip(&address.ip()) {
            refused = Some(address.ip());
            continue;
        }
        let stream = tokio::net::TcpStream::connect(address).await?;
        let _ = stream.set_nodelay(true);
        return Ok(stream);
    }
    let reason = match refused {
        Some(ip) => format!("{host} resolves to {ip}: refused (SSRF protection)"),
        None => format!("{host} resolves to no address"),
    };
    Err(Error::new(ErrorKind::PermissionDenied, reason))
}

async fn unary<Q, R>(
    channel: Channel,
    method: &str,
    request: Q,
    headers: &[(&str, String)],
) -> Result<R, NotPosted>
where
    Q: prost::Message + Send + Sync + 'static,
    R: prost::Message + Default + Send + Sync + 'static,
{
    let (mut grpc, request, path) = prepared(channel, method, request, headers).await?;
    let codec = tonic::codec::ProstCodec::<Q, R>::default();
    match grpc.unary(request, path, codec).await {
        Ok(reply) => Ok(reply.into_inner()),
        Err(status) => Err(refused(&status)),
    }
}

/// A call made ready: the client once the channel is, the request with the headers as its
/// metadata, the method's path.
async fn prepared<Q>(
    channel: Channel,
    method: &str,
    request: Q,
    headers: &[(&str, String)],
) -> Result<
    (
        tonic::client::Grpc<Channel>,
        tonic::Request<Q>,
        PathAndQuery,
    ),
    NotPosted,
> {
    let mut grpc = tonic::client::Grpc::new(channel);
    grpc.ready()
        .await
        .map_err(|error| NotPosted::Unsent(format!("gRPC: the agent is not ready: {error}")))?;
    let mut request = tonic::Request::new(request);
    for (name, value) in headers {
        let name = name.to_ascii_lowercase();
        let key = tonic::metadata::MetadataKey::from_bytes(name.as_bytes());
        if let (Ok(key), Ok(value)) = (key, value.parse()) {
            request.metadata_mut().insert(key, value);
        }
    }
    let path = PathAndQuery::try_from(format!("{SERVICE}{method}"))
        .map_err(|error| NotPosted::Unsent(error.to_string()))?;
    Ok((grpc, request, path))
}

/// A status as the other bindings' failures: a refusal (nothing happened) for the codes that
/// say the request was not taken, else its fate is unknown.
fn refused(status: &tonic::Status) -> NotPosted {
    use tonic::Code;
    let error = format!("gRPC {:?}: {}", status.code(), status.message());
    match status.code() {
        Code::InvalidArgument
        | Code::NotFound
        | Code::AlreadyExists
        | Code::PermissionDenied
        | Code::Unauthenticated
        | Code::FailedPrecondition
        | Code::OutOfRange
        | Code::Unimplemented => NotPosted::Unsent(error),
        _ => NotPosted::Unknown(error),
    }
}

// ---- JSON (as the JSON-RPC binding writes it) to the messages, and back

fn send_request(params: &Value) -> SendMessageRequest {
    let configuration = &params["configuration"];
    let push = &configuration["pushNotificationConfig"];
    SendMessageRequest {
        message: Some(message_of(&params["message"])),
        configuration: Some(SendMessageConfiguration {
            accepted_output_modes: strings(&configuration["acceptedOutputModes"]),
            task_push_notification_config: push.is_object().then(|| PushConfig {
                url: text(&push["url"]),
                token: text(&push["token"]),
            }),
            return_immediately: configuration["returnImmediately"] == true,
        }),
    }
}

fn message_of(message: &Value) -> Message {
    let parts = message["parts"].as_array().into_iter().flatten();
    Message {
        message_id: text(&message["messageId"]),
        context_id: text(&message["contextId"]),
        task_id: text(&message["taskId"]),
        role: named(&ROLES, &message["role"]),
        parts: parts.map(part_of).collect(),
        metadata: message["metadata"].as_object().map(to_struct),
    }
}

fn part_of(part: &Value) -> Part {
    let content = if let Some(text) = part["text"].as_str() {
        Some(part::Content::Text(text.to_owned()))
    } else if let Some(url) = part["url"].as_str() {
        Some(part::Content::Url(url.to_owned()))
    } else {
        part.get("data")
            .map(|data| part::Content::Data(to_proto(data)))
    };
    Part {
        content,
        filename: text(&part["filename"]),
        media_type: text(&part["mediaType"]),
    }
}

fn task_json(task: &Task) -> Value {
    let mut out = json!({ "id": task.id, "contextId": task.context_id });
    if let Some(status) = &task.status {
        let mut written = json!({ "state": STATES.get(status.state as usize).copied().unwrap_or("TASK_STATE_UNSPECIFIED") });
        if let Some(message) = &status.message {
            written["message"] = message_json(message);
        }
        out["status"] = written;
    }
    if !task.artifacts.is_empty() {
        let artifacts = task.artifacts.iter().map(|artifact| {
            json!({
                "artifactId": artifact.artifact_id,
                "name": artifact.name,
                "parts": artifact.parts.iter().map(part_json).collect::<Vec<_>>(),
            })
        });
        out["artifacts"] = artifacts.collect::<Vec<_>>().into();
    }
    out
}

/// A stream's event as the JSON-RPC binding's `result` of it.
fn stream_json(event: &StreamResponse) -> Value {
    use stream_response::Payload;
    let status = |status: &Option<TaskStatus>| {
        let task = Task {
            status: status.clone(),
            ..Task::default()
        };
        task_json(&task)["status"].take()
    };
    match &event.payload {
        Some(Payload::Task(task)) => json!({ "task": task_json(task) }),
        Some(Payload::Message(message)) => json!({ "message": message_json(message) }),
        Some(Payload::StatusUpdate(update)) => json!({ "statusUpdate": {
            "taskId": update.task_id, "contextId": update.context_id, "status": status(&update.status),
        }}),
        Some(Payload::ArtifactUpdate(update)) => {
            let artifact = update.artifact.clone().map(|artifact| Task {
                artifacts: vec![artifact],
                ..Task::default()
            });
            let artifact =
                artifact.map_or(Value::Null, |task| task_json(&task)["artifacts"][0].take());
            json!({ "artifactUpdate": {
                "taskId": update.task_id, "contextId": update.context_id, "artifact": artifact,
                "append": update.append, "lastChunk": update.last_chunk,
            }})
        }
        None => json!({}),
    }
}

/// An agent card as its JSON (A2A 1.0 ProtoJSON), with the fields the gateway reads.
fn card_json(card: &AgentCard) -> Value {
    use security_scheme::Scheme;
    let interfaces = card.supported_interfaces.iter().map(|interface| {
        json!({ "url": interface.url, "protocolBinding": interface.protocol_binding,
                "protocolVersion": interface.protocol_version })
    });
    let capabilities = card.capabilities.clone().unwrap_or_default();
    let mut flags = Map::new();
    for (key, flag) in [
        ("streaming", capabilities.streaming),
        ("pushNotifications", capabilities.push_notifications),
        ("extendedAgentCard", capabilities.extended_agent_card),
    ] {
        if let Some(flag) = flag {
            flags.insert(key.to_owned(), json!(flag));
        }
    }
    let schemes = card.security_schemes.iter().filter_map(|(name, scheme)| {
        let written = match scheme.scheme.as_ref()? {
            Scheme::ApiKey(key) => json!({ "apiKeySecurityScheme": {
                "location": key.location, "name": key.name } }),
            Scheme::Http(http) => json!({ "httpAuthSecurityScheme": { "scheme": http.scheme } }),
            Scheme::Oauth2(oauth2) => {
                let flow = oauth2
                    .flows
                    .as_ref()
                    .and_then(|f| f.client_credentials.as_ref());
                let flows = flow.map_or(
                    json!({}),
                    |flow| json!({ "clientCredentials": { "tokenUrl": flow.token_url } }),
                );
                json!({ "oauth2SecurityScheme": { "flows": flows } })
            }
        };
        Some((name.clone(), written))
    });
    let skills = card.skills.iter().map(
        |skill| json!({ "id": skill.id, "name": skill.name, "description": skill.description }),
    );
    json!({
        "name": card.name,
        "description": card.description,
        "version": card.version,
        "supportedInterfaces": interfaces.collect::<Vec<_>>(),
        "capabilities": flags,
        "securitySchemes": schemes.collect::<Map<_, _>>(),
        "skills": skills.collect::<Vec<_>>(),
    })
}

fn message_json(message: &Message) -> Value {
    let mut out = json!({
        "messageId": message.message_id,
        "role": ROLES.get(message.role as usize).copied().unwrap_or("ROLE_UNSPECIFIED"),
        "parts": message.parts.iter().map(part_json).collect::<Vec<_>>(),
    });
    for (key, value) in [
        ("contextId", &message.context_id),
        ("taskId", &message.task_id),
    ] {
        if !value.is_empty() {
            out[key] = json!(value);
        }
    }
    if let Some(metadata) = &message.metadata {
        out["metadata"] = from_struct(metadata);
    }
    out
}

fn part_json(part: &Part) -> Value {
    let mut out = match &part.content {
        Some(part::Content::Text(text)) => json!({ "text": text }),
        Some(part::Content::Url(url)) => json!({ "url": url }),
        Some(part::Content::Data(data)) => json!({ "data": from_proto(data) }),
        Some(part::Content::Raw(raw)) => json!({
            "raw": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, raw)
        }),
        None => json!({}),
    };
    for (key, value) in [
        ("filename", &part.filename),
        ("mediaType", &part.media_type),
    ] {
        if !value.is_empty() {
            out[key] = json!(value);
        }
    }
    out
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_owned()
}

fn strings(value: &Value) -> Vec<String> {
    let items = value.as_array().into_iter().flatten();
    items.filter_map(Value::as_str).map(str::to_owned).collect()
}

/// An enum's number from its ProtoJSON name; 0 (unspecified) for any other.
fn named(names: &[&str], value: &Value) -> i32 {
    let found = names.iter().position(|name| value.as_str() == Some(name));
    found.map_or(0, |n| n as i32)
}

fn to_proto(value: &Value) -> prost_types::Value {
    use prost_types::value::Kind;
    let kind = match value {
        Value::Null => Kind::NullValue(0),
        Value::Bool(flag) => Kind::BoolValue(*flag),
        Value::Number(number) => Kind::NumberValue(number.as_f64().unwrap_or_default()),
        Value::String(text) => Kind::StringValue(text.clone()),
        Value::Array(items) => Kind::ListValue(prost_types::ListValue {
            values: items.iter().map(to_proto).collect(),
        }),
        Value::Object(fields) => Kind::StructValue(to_struct(fields)),
    };
    prost_types::Value { kind: Some(kind) }
}

fn to_struct(fields: &Map<String, Value>) -> prost_types::Struct {
    prost_types::Struct {
        fields: fields
            .iter()
            .map(|(key, value)| (key.clone(), to_proto(value)))
            .collect(),
    }
}

fn from_struct(fields: &prost_types::Struct) -> Value {
    let fields = fields.fields.iter();
    Value::Object(
        fields
            .map(|(key, value)| (key.clone(), from_proto(value)))
            .collect(),
    )
}

/// A protobuf value as JSON. A whole number stays one (`expires_in_ms` is read as an integer).
fn from_proto(value: &prost_types::Value) -> Value {
    use prost_types::value::Kind;
    match &value.kind {
        None | Some(Kind::NullValue(_)) => Value::Null,
        Some(Kind::BoolValue(flag)) => json!(flag),
        Some(Kind::NumberValue(number))
            if number.fract() == 0.0 && number.abs() < 9_007_199_254_740_992.0 =>
        {
            json!(*number as i64)
        }
        Some(Kind::NumberValue(number)) => json!(number),
        Some(Kind::StringValue(text)) => json!(text),
        Some(Kind::ListValue(list)) => Value::Array(list.values.iter().map(from_proto).collect()),
        Some(Kind::StructValue(fields)) => from_struct(fields),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message as _;

    #[test]
    fn a_message_goes_out_and_a_task_comes_back_as_the_json_rpc_bindings_json() {
        let params = json!({
            "message": { "messageId": "m1", "role": "ROLE_USER", "taskId": "t1", "contextId": "c1",
                         "parts": [{ "text": "find a flight" },
                                   { "data": { "type": "control_applied", "outcome": "applied" } }],
                         "metadata": { "waav_state": { "version": 3 } } },
            "configuration": { "returnImmediately": true, "acceptedOutputModes": ["text/plain"],
                               "pushNotificationConfig": { "url": "https://gw/v1/tasks/t/callback",
                                                           "token": "ab" } },
        });
        let request = send_request(&params);
        // On the wire and back: what the agent decodes.
        let wire = request.encode_to_vec();
        let decoded = SendMessageRequest::decode(wire.as_slice()).expect("decoded");
        let message = decoded.message.expect("a message");
        assert_eq!((message.role, message.task_id.as_str()), (1, "t1"));
        assert_eq!(message_json(&message)["parts"], params["message"]["parts"]);
        assert_eq!(
            message_json(&message)["metadata"]["waav_state"]["version"],
            3
        );
        let configuration = decoded.configuration.expect("a configuration");
        assert!(configuration.return_immediately);
        let push = configuration.task_push_notification_config.expect("push");
        assert_eq!(
            (push.url.as_str(), push.token.as_str()),
            ("https://gw/v1/tasks/t/callback", "ab")
        );

        // A completed task with a text and a data artifact, and a status message.
        let task = Task {
            id: "t1".into(),
            context_id: "c1".into(),
            status: Some(TaskStatus {
                state: 3,
                message: Some(Message {
                    message_id: "s".into(),
                    role: 2,
                    parts: vec![part_of(&json!({ "text": "Done." }))],
                    ..Message::default()
                }),
            }),
            artifacts: vec![Artifact {
                artifact_id: "a".into(),
                name: "answer".into(),
                parts: vec![
                    part_of(&json!({ "text": "Flight UA 12 at 9:40." })),
                    part_of(&json!({ "data": { "seats": 2, "price": 99.5 } })),
                ],
            }],
        };
        let wire = SendMessageResponse {
            payload: Some(send_message_response::Payload::Task(task)),
        }
        .encode_to_vec();
        let reply = SendMessageResponse::decode(wire.as_slice()).expect("decoded");
        let Some(send_message_response::Payload::Task(task)) = reply.payload else {
            panic!("a task");
        };
        let written = task_json(&task);
        assert_eq!(written["status"]["state"], "TASK_STATE_COMPLETED");
        assert_eq!(written["status"]["message"]["role"], "ROLE_AGENT");
        assert_eq!(
            written["artifacts"][0]["parts"][0]["text"],
            "Flight UA 12 at 9:40."
        );
        assert_eq!(
            written["artifacts"][0]["parts"][1]["data"],
            json!({ "seats": 2, "price": 99.5 })
        );
        assert_eq!(
            STATES[named(&STATES, &json!("TASK_STATE_INPUT_REQUIRED")) as usize],
            "TASK_STATE_INPUT_REQUIRED"
        );
    }

    #[test]
    fn a_streams_events_and_an_extended_card_come_back_as_the_json_rpc_bindings_json() {
        use stream_response::Payload;
        let wire = |payload: Payload| {
            let encoded = StreamResponse {
                payload: Some(payload),
            }
            .encode_to_vec();
            stream_json(&StreamResponse::decode(encoded.as_slice()).expect("decoded"))
        };
        let working = TaskStatus {
            state: 2,
            message: Some(Message {
                message_id: "s1".into(),
                role: 2,
                parts: vec![part_of(&json!({ "text": "Looking." }))],
                ..Message::default()
            }),
        };
        let status = wire(Payload::StatusUpdate(TaskStatusUpdateEvent {
            task_id: "t1".into(),
            context_id: "c1".into(),
            status: Some(working),
        }));
        assert_eq!(status["statusUpdate"]["taskId"], "t1");
        assert_eq!(
            status["statusUpdate"]["status"]["state"],
            "TASK_STATE_WORKING"
        );
        assert_eq!(
            status["statusUpdate"]["status"]["message"]["parts"][0]["text"],
            "Looking."
        );
        let artifact = wire(Payload::ArtifactUpdate(TaskArtifactUpdateEvent {
            task_id: "t1".into(),
            artifact: Some(Artifact {
                artifact_id: "a".into(),
                name: "answer".into(),
                parts: vec![part_of(&json!({ "text": "Ten." }))],
            }),
            last_chunk: true,
            ..TaskArtifactUpdateEvent::default()
        }));
        assert_eq!(artifact["artifactUpdate"]["artifact"]["artifactId"], "a");
        assert_eq!(
            artifact["artifactUpdate"]["artifact"]["parts"][0]["text"],
            "Ten."
        );
        assert_eq!(artifact["artifactUpdate"]["lastChunk"], true);
        let task = wire(Payload::Task(Task {
            id: "t1".into(),
            ..Task::default()
        }));
        assert_eq!(task["task"]["id"], "t1");

        let card = AgentCard {
            name: "Desk".into(),
            description: "Answers.".into(),
            supported_interfaces: vec![AgentInterface {
                url: "https://agent.example.com:50051".into(),
                protocol_binding: "GRPC".into(),
                protocol_version: "1.0".into(),
            }],
            capabilities: Some(AgentCapabilities {
                streaming: Some(true),
                extended_agent_card: Some(true),
                ..AgentCapabilities::default()
            }),
            security_schemes: HashMap::from([
                (
                    "key".to_owned(),
                    SecurityScheme {
                        scheme: Some(security_scheme::Scheme::ApiKey(ApiKeyScheme {
                            location: "header".into(),
                            name: "X-Key".into(),
                        })),
                    },
                ),
                (
                    "oauth".to_owned(),
                    SecurityScheme {
                        scheme: Some(security_scheme::Scheme::Oauth2(OAuth2Scheme {
                            flows: Some(OAuthFlows {
                                client_credentials: Some(ClientCredentialsFlow {
                                    token_url: "https://auth.example.com/token".into(),
                                }),
                            }),
                        })),
                    },
                ),
            ]),
            skills: vec![AgentSkill {
                id: "museum".into(),
                name: "Museum hours".into(),
                description: "Opening times.".into(),
            }],
            ..AgentCard::default()
        };
        let encoded = card.encode_to_vec();
        let written = card_json(&AgentCard::decode(encoded.as_slice()).expect("decoded"));
        assert_eq!(written["name"], "Desk");
        assert_eq!(written["supportedInterfaces"][0]["protocolBinding"], "GRPC");
        assert_eq!(
            written["capabilities"],
            json!({ "streaming": true, "extendedAgentCard": true })
        );
        assert_eq!(
            written["securitySchemes"]["key"]["apiKeySecurityScheme"],
            json!({ "location": "header", "name": "X-Key" })
        );
        assert_eq!(
            written["securitySchemes"]["oauth"]["oauth2SecurityScheme"]["flows"]["clientCredentials"]
                ["tokenUrl"],
            "https://auth.example.com/token"
        );
        assert_eq!(written["skills"][0]["name"], "Museum hours");
    }

    /// Each connection of a channel is checked when it is made: a name that resolves to a
    /// private address now is refused even though the channel was made before.
    #[tokio::test]
    async fn a_connection_goes_only_to_an_address_the_ssrf_rules_allow() {
        // The escape hatch of the local mocks lets loopback through; the rule is tested off.
        if crate::core::net::loopback_endpoints_allowed() {
            return;
        }
        for target in [
            "http://127.0.0.1:9",
            "http://localhost:9",
            "https://[::1]:9",
        ] {
            let uri: tonic::transport::Uri = target.parse().unwrap();
            let refused = checked_connect(uri).await.expect_err(target);
            assert_eq!(
                refused.kind(),
                std::io::ErrorKind::PermissionDenied,
                "{target}"
            );
            assert!(refused.to_string().contains("SSRF"), "{target}: {refused}");
        }
    }

    #[test]
    fn a_refusal_is_told_from_a_call_whose_fate_is_unknown() {
        let unsent = refused(&tonic::Status::unauthenticated("no token"));
        assert!(
            matches!(&unsent, NotPosted::Unsent(text) if text.starts_with("gRPC Unauthenticated"))
        );
        assert!(matches!(
            refused(&tonic::Status::internal("x")),
            NotPosted::Unknown(_)
        ));
        assert!(matches!(
            refused(&tonic::Status::unavailable("x")),
            NotPosted::Unknown(_)
        ));
    }
}

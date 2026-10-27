#!/usr/bin/env python3
"""A stand-in A2A 1.0 agent on the gRPC binding (`lf.a2a.v1.A2AService`), for the live test's
PR7 scenarios: `SendMessage` (with `return_immediately`), `GetTask`, `CancelTask`,
`SendStreamingMessage` (the task working, a status update, the artifact, completed) and
`GetExtendedAgentCard` (a card whose name is `--extended-name`).

Its messages are declared at run time with the field numbers of the A2A specification's
`a2a.proto` (only the fields the gateway reads or sends), so no generated code is needed:
it needs `grpcio` and `protobuf` only (a venv of its own; the live script's interpreter has
no grpcio). Every call needs `authorization: Bearer <token>`; each is written as one JSON line
to `--log` (method, whether the token was right, the message's text). A task works for
`--answer-after` seconds, then completes with one text artifact, `--answer`.

    python3 a2a_grpc_standin.py --port 50951 --token T --log hits.jsonl --answer "..."
"""

import argparse
import json
import threading
import time
import uuid
from concurrent import futures

import grpc
from google.protobuf import descriptor_pb2, descriptor_pool, message_factory, struct_pb2  # noqa: F401

STRING, INT32, BOOL, MESSAGE, BYTES = 9, 5, 8, 11, 12
OPTIONAL, REPEATED = 1, 3


def declare():
    file = descriptor_pb2.FileDescriptorProto(name="a2a_grpc_standin.proto", package="lf.a2a.v1",
                                              syntax="proto3")
    file.dependency.append("google/protobuf/struct.proto")

    def message(name, fields, oneofs=()):
        declared = file.message_type.add(name=name)
        for oneof in oneofs:
            declared.oneof_decl.add(name=oneof)
        for field in fields:
            fname, number, kind, label = field[:4]
            type_name = field[4] if len(field) > 4 else None
            oneof = field[5] if len(field) > 5 else None
            added = declared.field.add(name=fname, number=number, type=kind, label=label)
            if type_name:
                added.type_name = type_name
            if oneof is not None:
                added.oneof_index = oneof

    message("Part", [("text", 1, STRING, OPTIONAL, None, 0), ("raw", 2, BYTES, OPTIONAL, None, 0),
                     ("url", 3, STRING, OPTIONAL, None, 0),
                     ("data", 4, MESSAGE, OPTIONAL, ".google.protobuf.Value", 0),
                     ("filename", 6, STRING, OPTIONAL), ("media_type", 7, STRING, OPTIONAL)], ["content"])
    message("Message", [("message_id", 1, STRING, OPTIONAL), ("context_id", 2, STRING, OPTIONAL),
                        ("task_id", 3, STRING, OPTIONAL), ("role", 4, INT32, OPTIONAL),
                        ("parts", 5, MESSAGE, REPEATED, ".lf.a2a.v1.Part"),
                        ("metadata", 6, MESSAGE, OPTIONAL, ".google.protobuf.Struct")])
    message("TaskStatus", [("state", 1, INT32, OPTIONAL), ("message", 2, MESSAGE, OPTIONAL, ".lf.a2a.v1.Message")])
    message("Artifact", [("artifact_id", 1, STRING, OPTIONAL), ("name", 2, STRING, OPTIONAL),
                         ("parts", 4, MESSAGE, REPEATED, ".lf.a2a.v1.Part")])
    message("Task", [("id", 1, STRING, OPTIONAL), ("context_id", 2, STRING, OPTIONAL),
                     ("status", 3, MESSAGE, OPTIONAL, ".lf.a2a.v1.TaskStatus"),
                     ("artifacts", 4, MESSAGE, REPEATED, ".lf.a2a.v1.Artifact")])
    message("PushConfig", [("url", 4, STRING, OPTIONAL), ("token", 5, STRING, OPTIONAL)])
    message("SendMessageConfiguration", [("accepted_output_modes", 1, STRING, REPEATED),
                                         ("task_push_notification_config", 2, MESSAGE, OPTIONAL,
                                          ".lf.a2a.v1.PushConfig"),
                                         ("return_immediately", 4, BOOL, OPTIONAL)])
    message("SendMessageRequest", [("message", 2, MESSAGE, OPTIONAL, ".lf.a2a.v1.Message"),
                                   ("configuration", 3, MESSAGE, OPTIONAL, ".lf.a2a.v1.SendMessageConfiguration")])
    message("SendMessageResponse", [("task", 1, MESSAGE, OPTIONAL, ".lf.a2a.v1.Task", 0),
                                    ("message", 2, MESSAGE, OPTIONAL, ".lf.a2a.v1.Message", 0)], ["payload"])
    message("TaskRequest", [("id", 2, STRING, OPTIONAL)])
    message("TaskStatusUpdateEvent", [("task_id", 1, STRING, OPTIONAL), ("context_id", 2, STRING, OPTIONAL),
                                      ("status", 3, MESSAGE, OPTIONAL, ".lf.a2a.v1.TaskStatus")])
    message("TaskArtifactUpdateEvent", [("task_id", 1, STRING, OPTIONAL), ("context_id", 2, STRING, OPTIONAL),
                                        ("artifact", 3, MESSAGE, OPTIONAL, ".lf.a2a.v1.Artifact"),
                                        ("append", 4, BOOL, OPTIONAL), ("last_chunk", 5, BOOL, OPTIONAL)])
    message("StreamResponse", [("task", 1, MESSAGE, OPTIONAL, ".lf.a2a.v1.Task", 0),
                               ("message", 2, MESSAGE, OPTIONAL, ".lf.a2a.v1.Message", 0),
                               ("status_update", 3, MESSAGE, OPTIONAL, ".lf.a2a.v1.TaskStatusUpdateEvent", 0),
                               ("artifact_update", 4, MESSAGE, OPTIONAL, ".lf.a2a.v1.TaskArtifactUpdateEvent", 0)],
            ["payload"])
    message("GetExtendedAgentCardRequest", [("tenant", 1, STRING, OPTIONAL)])
    message("AgentInterface", [("url", 1, STRING, OPTIONAL), ("protocol_binding", 2, STRING, OPTIONAL),
                               ("protocol_version", 4, STRING, OPTIONAL)])
    message("AgentCapabilities", [("streaming", 1, BOOL, OPTIONAL), ("push_notifications", 2, BOOL, OPTIONAL),
                                  ("extended_agent_card", 4, BOOL, OPTIONAL)])
    message("AgentSkill", [("id", 1, STRING, OPTIONAL), ("name", 2, STRING, OPTIONAL),
                           ("description", 3, STRING, OPTIONAL)])
    message("AgentCard", [("name", 1, STRING, OPTIONAL), ("description", 2, STRING, OPTIONAL),
                          ("supported_interfaces", 3, MESSAGE, REPEATED, ".lf.a2a.v1.AgentInterface"),
                          ("version", 5, STRING, OPTIONAL),
                          ("capabilities", 7, MESSAGE, OPTIONAL, ".lf.a2a.v1.AgentCapabilities"),
                          ("skills", 12, MESSAGE, REPEATED, ".lf.a2a.v1.AgentSkill")])
    pool = descriptor_pool.Default()
    pool.Add(file)
    return {name: message_factory.GetMessageClass(pool.FindMessageTypeByName(f"lf.a2a.v1.{name}"))
            for name in ("Part", "Message", "TaskStatus", "Artifact", "Task", "SendMessageRequest",
                         "SendMessageResponse", "TaskRequest", "StreamResponse", "GetExtendedAgentCardRequest",
                         "AgentCard")}


WORKING, COMPLETED, CANCELED = 2, 3, 5


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--token", required=True)
    parser.add_argument("--log", required=True)
    parser.add_argument("--answer", default="The gRPC agent has the answer.")
    parser.add_argument("--answer-after", type=float, default=1.5)
    parser.add_argument("--extended-name", default="Museum desk (extended)")
    args = parser.parse_args()
    types = declare()
    tasks = {}
    lock = threading.Lock()

    def note(method, context, **more):
        metadata = dict(context.invocation_metadata())
        authorised = metadata.get("authorization") == f"Bearer {args.token}"
        with lock, open(args.log, "a") as log:
            log.write(json.dumps({"t": time.time(), "method": method, "authorised": authorised,
                                  "a2a_version": metadata.get("a2a-version"), **more}) + "\n")
        if not authorised:
            context.abort(grpc.StatusCode.UNAUTHENTICATED, "no or a wrong bearer token")

    def task_of(job):
        task = types["Task"](id=job["id"], context_id=job["context"])
        task.status.state = job["state"]
        if job["state"] == WORKING:
            status = task.status.message
            status.message_id, status.role = uuid.uuid4().hex, 2
            status.parts.add(text="Looking it up over gRPC.")
        if job["state"] == COMPLETED:
            artifact = task.artifacts.add(artifact_id="answer", name="answer")
            artifact.parts.add(text=args.answer)
        return task

    def advance(job):
        if job["state"] == WORKING and time.time() - job["created"] >= args.answer_after:
            job["state"] = COMPLETED

    def send(request, context):
        text = " ".join(part.text for part in request.message.parts if part.HasField("text"))
        note("SendMessage", context, text=text, task_id=request.message.task_id,
             return_immediately=request.configuration.return_immediately)
        job = {"id": f"g-{uuid.uuid4().hex[:8]}", "context": request.message.context_id or f"c-{uuid.uuid4().hex[:6]}",
               "state": WORKING, "created": time.time()}
        with lock:
            tasks[job["id"]] = job
        return types["SendMessageResponse"](task=task_of(job))

    def get(request, context):
        note("GetTask", context, id=request.id)
        with lock:
            job = tasks.get(request.id)
        if job is None:
            context.abort(grpc.StatusCode.NOT_FOUND, "no such task")
        advance(job)
        return task_of(job)

    def cancel(request, context):
        note("CancelTask", context, id=request.id)
        with lock:
            job = tasks.get(request.id)
        if job is None:
            context.abort(grpc.StatusCode.NOT_FOUND, "no such task")
        job["state"] = CANCELED
        return task_of(job)

    def stream(request, context):
        text = " ".join(part.text for part in request.message.parts if part.HasField("text"))
        note("SendStreamingMessage", context, text=text, task_id=request.message.task_id)
        job = {"id": f"g-{uuid.uuid4().hex[:8]}", "context": request.message.context_id or f"c-{uuid.uuid4().hex[:6]}",
               "state": WORKING, "created": time.time()}
        with lock:
            tasks[job["id"]] = job
        yield types["StreamResponse"](task=task_of(job))
        time.sleep(args.answer_after / 2)
        working = types["StreamResponse"]()
        working.status_update.task_id, working.status_update.context_id = job["id"], job["context"]
        working.status_update.status.CopyFrom(task_of(job).status)
        yield working
        time.sleep(args.answer_after / 2)
        job["state"] = COMPLETED
        done = task_of(job)
        artifact = types["StreamResponse"]()
        artifact.artifact_update.task_id, artifact.artifact_update.context_id = job["id"], job["context"]
        artifact.artifact_update.artifact.CopyFrom(done.artifacts[0])
        artifact.artifact_update.last_chunk = True
        yield artifact
        completed = types["StreamResponse"]()
        completed.status_update.task_id, completed.status_update.context_id = job["id"], job["context"]
        completed.status_update.status.CopyFrom(done.status)
        yield completed

    def extended(request, context):
        note("GetExtendedAgentCard", context)
        card = types["AgentCard"](name=args.extended_name, description="Answers over gRPC, for its clients.",
                                  version="1.0")
        card.supported_interfaces.add(url=f"http://127.0.0.1:{args.port}", protocol_binding="GRPC",
                                      protocol_version="1.0")
        card.capabilities.streaming = True
        card.skills.add(id="hours", name="Opening hours", description="When the museum opens")
        return card

    def unary(handler, request_type):
        return grpc.unary_unary_rpc_method_handler(handler, request_deserializer=types[request_type].FromString,
                                                   response_serializer=lambda message: message.SerializeToString())

    server = grpc.server(futures.ThreadPoolExecutor(max_workers=8))
    server.add_generic_rpc_handlers((grpc.method_handlers_generic_handler("lf.a2a.v1.A2AService", {
        "SendMessage": unary(send, "SendMessageRequest"),
        "GetTask": unary(get, "TaskRequest"),
        "CancelTask": unary(cancel, "TaskRequest"),
        "SendStreamingMessage": grpc.unary_stream_rpc_method_handler(
            stream, request_deserializer=types["SendMessageRequest"].FromString,
            response_serializer=lambda message: message.SerializeToString()),
        "GetExtendedAgentCard": unary(extended, "GetExtendedAgentCardRequest"),
    }),))
    server.add_insecure_port(f"127.0.0.1:{args.port}")
    server.start()
    print(f"a2a grpc stand-in on 127.0.0.1:{args.port}", flush=True)
    server.wait_for_termination()


if __name__ == "__main__":
    main()

# syntax=docker/dockerfile:1
#
# The front model server of interaction mode: a pinned llama.cpp server with Qwen3.5-2B Q4_K_M baked
# in, so a pod starts without a download. It is what `front_model.url` / `front_model.urls` point at
# when the operator runs the front (gateway/docs/interaction_mode.md, "A Shared Front"). The model is the one
# the front prompt, grammar and evals here are written and measured for, so a change to either is
# made in the same PR.
#
# One file, two devices: --build-arg DEVICE=cpu (the default) or DEVICE=cuda, published as
# registry.bud.studio/runtime/waav-front:<LLAMA_CPP_BUILD>-cpu / -cuda.
#
# Built and pushed by .github/workflows/front-image.yml. Bump LLAMA_CPP_BUILD or the model pin by PR;
# the release job refuses to overwrite a tag that already exists. The model is read from its Hugging
# Face revision, never `main`, and its sha256 is checked before it is copied in. Qwen3.5-2B is
# Apache-2.0; llama.cpp is MIT.

ARG LLAMA_CPP_BUILD=b11206
ARG DEVICE=cpu

FROM curlimages/curl:8.10.1 AS model
ARG MODEL_REPO=unsloth/Qwen3.5-2B-GGUF
ARG MODEL_REVISION=f6d5376be1edb4d416d56da11e5397a961aca8ae
ARG MODEL_FILE=Qwen3.5-2B-Q4_K_M.gguf
ARG MODEL_SHA256=aaf42c8b7c3cab2bf3d69c355048d4a0ee9973d48f16c731c0520ee914699223
WORKDIR /tmp/model
RUN curl -fsSL --retry 5 --retry-all-errors -o "${MODEL_FILE}" \
      "https://huggingface.co/${MODEL_REPO}/resolve/${MODEL_REVISION}/${MODEL_FILE}" \
 && echo "${MODEL_SHA256}  ${MODEL_FILE}" | sha256sum -c -

FROM ghcr.io/ggml-org/llama.cpp:server-${LLAMA_CPP_BUILD} AS base-cpu
FROM ghcr.io/ggml-org/llama.cpp:server-cuda-${LLAMA_CPP_BUILD} AS base-cuda

FROM base-${DEVICE}
ARG LLAMA_CPP_BUILD
ARG DEVICE
ARG MODEL_REPO=unsloth/Qwen3.5-2B-GGUF
ARG MODEL_REVISION=f6d5376be1edb4d416d56da11e5397a961aca8ae
ARG MODEL_FILE=Qwen3.5-2B-Q4_K_M.gguf
ARG MODEL_SHA256=aaf42c8b7c3cab2bf3d69c355048d4a0ee9973d48f16c731c0520ee914699223
COPY --from=model /tmp/model/${MODEL_FILE} /models/${MODEL_FILE}
# llama-server reads every flag from LLAMA_ARG_* as well: a deployment passes only the serving flags.
ENV LLAMA_ARG_MODEL=/models/${MODEL_FILE}
LABEL org.opencontainers.image.title="waav-front" \
      org.opencontainers.image.description="llama.cpp server with the front model of WaaV interaction mode" \
      org.opencontainers.image.source="https://github.com/BudEcosystem/WaaV" \
      org.opencontainers.image.licenses="MIT AND Apache-2.0" \
      studio.bud.waav-front.device="${DEVICE}" \
      studio.bud.waav-front.llama-cpp-build="${LLAMA_CPP_BUILD}" \
      studio.bud.waav-front.model="${MODEL_REPO}/${MODEL_FILE}@${MODEL_REVISION}" \
      studio.bud.waav-front.model-sha256="${MODEL_SHA256}"

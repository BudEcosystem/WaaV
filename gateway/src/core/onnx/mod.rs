//! Hardware execution-provider (EP) policy for ONNX sessions (MASTER_PLAN §7 "H").
//!
//! This is the **single policy point** for hardware acceleration: every ONNX
//! session built by the gateway (turn_detect `ModelManager`, smart_turn
//! `SmartTurnDetector`, silero_vad `SileroVAD`) pipes its `SessionBuilder`
//! through [`apply_execution_providers`] before committing the model.
//! Portability invariant: **no EP-specific code exists outside this module**.
//!
//! # Environment contract
//!
//! `WAAV_ORT_EP` = `auto` | `cpu` | `cuda` | `tensorrt` | `coreml` | `directml`
//! | `xnnpack` (default: `auto`, case-insensitive; empty string == unset).
//!
//! - `cpu` — return the builder unchanged (ONNX Runtime's implicit CPU EP).
//! - explicit EP name — register exactly that EP. If it is unavailable in the
//!   loaded ONNX Runtime build (or registration fails), `warn!` with the error,
//!   emit `waav_degraded_total{component="ort_ep", reason="requested_unavailable"}`,
//!   and fall back to CPU.
//! - `auto` — probe a platform-sensible order (Linux: cuda → xnnpack; macOS:
//!   coreml; Windows: directml → cuda) via `ExecutionProvider::is_available()`
//!   and register the first EP that accepts; otherwise CPU.
//! - unknown value — fail session creation with an `ort::Error`. This is a
//!   malformed deploy, not an accelerator availability problem, so it must be
//!   visible instead of silently changing policy.
//!
//! **CPU is always the final fallback for missing accelerators.** This function
//! never fails session creation because an accelerator is unavailable —
//! accelerator problems degrade to CPU with a `warn!` and a metric. Malformed
//! operator configuration returns an `Err`.
//!
//! The active EP is published on the `waav_ort_ep{ep}` gauge (1 for the EP in
//! use, 0 for the rest; bounded label set).
//!
//! # What is compilable today
//!
//! The gateway builds `ort` with the `load-dynamic` feature (see
//! `Cargo.toml`), under which **every** EP's registration path compiles and
//! the per-EP cargo features (`ort/cuda`, `ort/coreml`, …) are not required:
//! whether an EP actually works is decided at runtime by the
//! `libonnxruntime.so` loaded via `ORT_DYLIB_PATH` (a CPU-only runtime simply
//! reports every accelerator as unavailable and we fall back). If the build
//! ever switches to static linking, `ExecutionProvider::register` returns
//! `RegisterError::MissingFeature` for EPs whose cargo feature is absent —
//! that error flows through the same warn-and-fall-back path, so the policy
//! and env contract hold regardless.

use ort::execution_providers::{
    CUDAExecutionProvider, CoreMLExecutionProvider, DirectMLExecutionProvider, ExecutionProvider,
    TensorRTExecutionProvider, XNNPACKExecutionProvider,
};
use ort::session::builder::SessionBuilder;
use tracing::{debug, error, info, warn};

use crate::core::metrics::bridge::record_degraded;

/// Environment variable selecting the ONNX Runtime execution provider.
pub const WAAV_ORT_EP_ENV: &str = "WAAV_ORT_EP";

/// Bounded label set for the `waav_ort_ep` gauge.
const EP_GAUGE_LABELS: [&str; 6] = ["cpu", "cuda", "tensorrt", "coreml", "directml", "xnnpack"];

/// Hardware execution providers this policy knows how to request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EpKind {
    Cuda,
    TensorRt,
    CoreMl,
    DirectMl,
    Xnnpack,
}

impl EpKind {
    /// The env-value / metric-label spelling of this EP.
    fn label(self) -> &'static str {
        match self {
            EpKind::Cuda => "cuda",
            EpKind::TensorRt => "tensorrt",
            EpKind::CoreMl => "coreml",
            EpKind::DirectMl => "directml",
            EpKind::Xnnpack => "xnnpack",
        }
    }

    /// Build the ort provider with default options.
    ///
    /// Per-model thread tuning stays at the session level (tiny models run
    /// sequential / 1 intra-op); big-model knobs (TensorRT workspace, FP16, …)
    /// are deliberately not set here — defaults only, extras can be threaded
    /// through later without touching call sites.
    fn provider(self) -> Box<dyn ExecutionProvider> {
        match self {
            EpKind::Cuda => Box::new(CUDAExecutionProvider::default()),
            EpKind::TensorRt => Box::new(TensorRTExecutionProvider::default()),
            EpKind::CoreMl => Box::new(CoreMLExecutionProvider::default()),
            EpKind::DirectMl => Box::new(DirectMLExecutionProvider::default()),
            EpKind::Xnnpack => Box::new(XNNPACKExecutionProvider::default()),
        }
    }
}

/// A parsed `WAAV_ORT_EP` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EpRequest {
    Auto,
    Cpu,
    Explicit(EpKind),
}

/// Parse a `WAAV_ORT_EP` value. `None` means the value is unrecognized.
/// Unset/empty/whitespace parses as the default, `Auto`.
fn parse_request(raw: &str) -> Option<EpRequest> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "" | "auto" => Some(EpRequest::Auto),
        "cpu" => Some(EpRequest::Cpu),
        "cuda" => Some(EpRequest::Explicit(EpKind::Cuda)),
        "tensorrt" => Some(EpRequest::Explicit(EpKind::TensorRt)),
        "coreml" => Some(EpRequest::Explicit(EpKind::CoreMl)),
        "directml" => Some(EpRequest::Explicit(EpKind::DirectMl)),
        "xnnpack" => Some(EpRequest::Explicit(EpKind::Xnnpack)),
        _ => None,
    }
}

fn invalid_ep_error(value: &str) -> ort::Error {
    ort::Error::new(format!(
        "{WAAV_ORT_EP_ENV} must be one of auto|cpu|cuda|tensorrt|coreml|directml|xnnpack; got {value:?}"
    ))
}

fn request_from_env() -> ort::Result<EpRequest> {
    match std::env::var(WAAV_ORT_EP_ENV) {
        Ok(raw) => parse_request(&raw).ok_or_else(|| invalid_ep_error(&raw)),
        Err(std::env::VarError::NotPresent) => Ok(EpRequest::Auto),
        Err(std::env::VarError::NotUnicode(value)) => {
            let message =
                format!("{WAAV_ORT_EP_ENV} must be valid UTF-8; got non-unicode value {value:?}");
            Err(ort::Error::new(message))
        }
    }
}

/// Platform-sensible probe order for `WAAV_ORT_EP=auto`.
fn auto_probe_order() -> &'static [EpKind] {
    if cfg!(target_os = "macos") {
        &[EpKind::CoreMl]
    } else if cfg!(target_os = "windows") {
        &[EpKind::DirectMl, EpKind::Cuda]
    } else {
        // Linux (and everything else): CUDA first, then XNNPACK (present in
        // some CPU-oriented ORT builds on aarch64/x86_64).
        &[EpKind::Cuda, EpKind::Xnnpack]
    }
}

/// Publish the active EP on the `waav_ort_ep{ep}` gauge (1 = active, 0 = not).
fn publish_ep_gauge(active: &'static str) {
    for label in EP_GAUGE_LABELS {
        let value = if label == active { 1.0 } else { 0.0 };
        metrics::gauge!("waav_ort_ep", "ep" => label).set(value);
    }
}

/// Probe and register `kind` on `builder`. Returns `true` iff the EP was
/// accepted by ONNX Runtime. Never propagates an error: failures are logged
/// (`warn!` for failures the operator should see, `debug!` for routine
/// auto-probe misses) and reported as `false` so the caller can fall back.
fn try_register(kind: EpKind, builder: &mut SessionBuilder, explicit: bool) -> bool {
    let ep = kind.provider();

    // `is_available()` asks the *loaded* ONNX Runtime which providers it was
    // built with — the authoritative runtime answer under `load-dynamic`.
    match ep.is_available() {
        Ok(true) => {}
        Ok(false) => {
            if explicit {
                warn!(
                    ep = kind.label(),
                    "WAAV_ORT_EP requested an execution provider that the loaded ONNX Runtime \
                     build does not provide; falling back to CPU"
                );
            } else {
                debug!(
                    ep = kind.label(),
                    "execution provider not available in this ONNX Runtime build; skipping"
                );
            }
            return false;
        }
        Err(e) => {
            warn!(
                ep = kind.label(),
                error = %e,
                "execution provider availability probe failed; treating as unavailable"
            );
            return false;
        }
    }

    match ep.register(builder) {
        Ok(()) => {
            info!(
                ep = kind.label(),
                provider = ep.name(),
                "registered ONNX execution provider"
            );
            true
        }
        Err(e) => {
            warn!(
                ep = kind.label(),
                error = %e,
                "execution provider registration failed; falling back"
            );
            false
        }
    }
}

/// Apply the gateway-wide hardware EP policy to an ONNX `SessionBuilder`.
///
/// Reads [`WAAV_ORT_EP_ENV`] (default `auto`) and registers at most one
/// hardware execution provider on the builder; see the module docs for the
/// full contract. CPU is the guaranteed fallback for unavailable accelerator
/// providers; malformed explicit `WAAV_ORT_EP` values fail before builder
/// mutation.
/// The `ort::Result` return keeps call sites uniform (`?`-chained between the
/// other builder options).
pub fn apply_execution_providers(mut builder: SessionBuilder) -> ort::Result<SessionBuilder> {
    let request = request_from_env().map_err(|e| {
        error!(error = %e, "invalid ONNX execution-provider environment configuration");
        e
    })?;

    match request {
        EpRequest::Cpu => {
            debug!("WAAV_ORT_EP=cpu: using CPU execution provider only");
            publish_ep_gauge("cpu");
        }
        EpRequest::Explicit(kind) => {
            if try_register(kind, &mut builder, true) {
                publish_ep_gauge(kind.label());
            } else {
                record_degraded("ort_ep", "requested_unavailable");
                publish_ep_gauge("cpu");
            }
        }
        EpRequest::Auto => {
            let mut registered = false;
            for kind in auto_probe_order() {
                if try_register(*kind, &mut builder, false) {
                    publish_ep_gauge(kind.label());
                    registered = true;
                    break;
                }
            }
            if !registered {
                debug!("WAAV_ORT_EP=auto: no hardware execution provider available; using CPU");
                publish_ep_gauge("cpu");
            }
        }
    }

    Ok(builder)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    // The SessionBuilder-based tests construct a real ort SessionBuilder,
    // which under `load-dynamic` requires a loadable ONNX Runtime — run with
    // ORT_DYLIB_PATH pointing at a libonnxruntime.so (any CPU build works;
    // these tests only exercise fallback paths and never need an accelerator).

    fn set_ep(value: &str) {
        // SAFETY: test-only env mutation, serialized via #[serial] (same
        // pattern as the src/config env-var tests).
        unsafe { std::env::set_var(WAAV_ORT_EP_ENV, value) };
    }

    fn clear_ep() {
        // SAFETY: see set_ep.
        unsafe { std::env::remove_var(WAAV_ORT_EP_ENV) };
    }

    fn builder() -> SessionBuilder {
        SessionBuilder::new()
            .expect("ONNX Runtime must be loadable for this test (set ORT_DYLIB_PATH)")
    }

    #[test]
    fn parse_request_recognizes_contract_values() {
        assert_eq!(parse_request("auto"), Some(EpRequest::Auto));
        assert_eq!(parse_request(""), Some(EpRequest::Auto));
        assert_eq!(parse_request("  AUTO "), Some(EpRequest::Auto));
        assert_eq!(parse_request("cpu"), Some(EpRequest::Cpu));
        assert_eq!(parse_request("CPU"), Some(EpRequest::Cpu));
        assert_eq!(
            parse_request("cuda"),
            Some(EpRequest::Explicit(EpKind::Cuda))
        );
        assert_eq!(
            parse_request("tensorrt"),
            Some(EpRequest::Explicit(EpKind::TensorRt))
        );
        assert_eq!(
            parse_request("coreml"),
            Some(EpRequest::Explicit(EpKind::CoreMl))
        );
        assert_eq!(
            parse_request("directml"),
            Some(EpRequest::Explicit(EpKind::DirectMl))
        );
        assert_eq!(
            parse_request("xnnpack"),
            Some(EpRequest::Explicit(EpKind::Xnnpack))
        );
        assert_eq!(parse_request("warp-drive"), None);
        assert_eq!(parse_request("gpu"), None);
    }

    #[test]
    #[serial]
    fn request_from_env_rejects_unknown_value() {
        set_ep("warp-drive");
        let result = request_from_env();
        clear_ep();
        let err = result.expect_err("unknown WAAV_ORT_EP must fail strict env parsing");
        let msg = err.to_string();
        assert!(msg.contains(WAAV_ORT_EP_ENV), "error names env var: {msg}");
        assert!(
            msg.contains("warp-drive"),
            "error includes malformed value: {msg}"
        );
    }

    #[test]
    fn auto_probe_order_is_nonempty_on_supported_platforms() {
        assert!(
            !auto_probe_order().is_empty(),
            "auto probe order must list at least one candidate EP on tier-1 platforms"
        );
    }

    #[test]
    #[serial]
    fn cpu_returns_builder_ok() {
        set_ep("cpu");
        let result = apply_execution_providers(builder());
        clear_ep();
        assert!(result.is_ok(), "WAAV_ORT_EP=cpu must never fail");
    }

    #[test]
    #[serial]
    fn unknown_value_fails_before_cpu_fallback() {
        set_ep("warp-drive");
        let result = apply_execution_providers(builder());
        clear_ep();
        assert!(
            result.is_err(),
            "unknown WAAV_ORT_EP values must fail instead of silently using CPU"
        );
    }

    #[test]
    #[serial]
    fn unset_env_defaults_to_auto_and_never_fails() {
        clear_ep();
        let result = apply_execution_providers(builder());
        assert!(
            result.is_ok(),
            "auto probing must always succeed (CPU is the final fallback)"
        );
    }

    #[test]
    #[serial]
    fn explicit_unavailable_ep_falls_back_to_cpu() {
        // DirectML is Windows-only, CoreML is Apple-only — on any given test
        // runtime at least one of these is guaranteed unavailable, which
        // exercises the warn + record_degraded + CPU-fallback path.
        let unavailable = if cfg!(target_os = "windows") {
            "coreml"
        } else {
            "directml"
        };
        set_ep(unavailable);
        let result = apply_execution_providers(builder());
        clear_ep();
        assert!(
            result.is_ok(),
            "an explicitly requested but unavailable EP must degrade to CPU, not fail"
        );
    }
}

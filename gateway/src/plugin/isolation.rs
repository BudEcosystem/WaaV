//! Plugin Isolation and Panic Safety
//!
//! This module provides panic isolation for plugin calls using `catch_unwind`.
//! Panics in plugin code are caught and converted to errors, preventing
//! plugin failures from crashing the gateway.
//!
//! # Safety Considerations
//!
//! - `catch_unwind` only catches panics, not aborts
//! - Plugins must not use `panic = "abort"` in their Cargo.toml
//! - FFI boundaries require additional care (panics across FFI are UB)

use std::any::Any;
use std::panic::{AssertUnwindSafe, UnwindSafe, catch_unwind};

/// Plugin-specific error type
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    /// Plugin panicked during execution
    #[error("Plugin panicked: {0}")]
    Panic(String),

    /// Plugin initialization failed
    #[error("Plugin initialization failed: {0}")]
    InitializationFailed(String),

    /// Plugin configuration error
    #[error("Plugin configuration error: {0}")]
    ConfigurationError(String),

    /// Plugin not found
    #[error("Plugin not found: {0}")]
    NotFound(String),

    /// Plugin version incompatible
    #[error("Plugin version incompatible: {0}")]
    VersionIncompatible(String),

    /// Plugin dependency missing
    #[error("Plugin dependency missing: {0}")]
    DependencyMissing(String),

    /// Plugin internal error
    #[error("Plugin internal error: {0}")]
    InternalError(String),
}

/// Safely call a plugin function with panic catching
///
/// This function wraps the plugin call in `catch_unwind` to prevent panics
/// from propagating to the caller. If the plugin panics, the panic is
/// converted to a `PluginError::Panic`.
///
/// # Type Parameters
///
/// - `F`: The function to call (must be UnwindSafe)
/// - `T`: The return type on success
/// - `E`: The error type that can be converted to PluginError
///
/// # Example
///
/// ```ignore
/// let result = call_plugin_safely(|| {
///     provider.create_stt(config)
/// })?;
/// ```
pub fn call_plugin_safely<F, T, E>(plugin_fn: F) -> Result<T, PluginError>
where
    F: FnOnce() -> Result<T, E> + UnwindSafe,
    E: std::fmt::Display,
{
    match catch_unwind(plugin_fn) {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(e)) => Err(PluginError::InternalError(e.to_string())),
        Err(panic_info) => {
            let msg = extract_panic_message(&panic_info);
            tracing::error!(message = %msg, "Plugin panicked");
            Err(PluginError::Panic(msg))
        }
    }
}

/// Safely call a plugin function, preserving the original error type
///
/// Unlike `call_plugin_safely`, this function preserves the original error type
/// from the plugin call. Panics are still caught and converted to a string error
/// using the provided converter function.
///
/// # Type Parameters
///
/// - `F`: The function to call (must be UnwindSafe)
/// - `T`: The return type on success
/// - `E`: The error type (preserved on normal errors)
/// - `PanicConverter`: Function to convert panic message to error type
pub fn call_plugin_preserving_error<F, T, E, PC>(plugin_fn: F, panic_to_error: PC) -> Result<T, E>
where
    F: FnOnce() -> Result<T, E> + UnwindSafe,
    PC: FnOnce(String) -> E,
{
    match catch_unwind(plugin_fn) {
        Ok(result) => result, // Preserve the original Result
        Err(panic_info) => {
            let msg = extract_panic_message(&panic_info);
            tracing::error!(message = %msg, "Plugin panicked");
            Err(panic_to_error(msg))
        }
    }
}

/// Safely call a plugin function that returns a value directly (no Result)
///
/// Use this for plugin functions that don't return a Result type.
pub fn call_plugin_safely_value<F, T>(plugin_fn: F) -> Result<T, PluginError>
where
    F: FnOnce() -> T + UnwindSafe,
{
    match catch_unwind(plugin_fn) {
        Ok(result) => Ok(result),
        Err(panic_info) => {
            let msg = extract_panic_message(&panic_info);
            tracing::error!(message = %msg, "Plugin panicked");
            Err(PluginError::Panic(msg))
        }
    }
}

/// Safely call an async plugin function with panic catching
///
/// This wraps the future in `AssertUnwindSafe` and catches panics during
/// both future creation AND polling. Uses `FutureExt::catch_unwind` from
/// the futures crate internally via a manual implementation.
///
/// # Example
///
/// ```ignore
/// let result = call_plugin_safely_async(|| async {
///     provider.process_audio(data).await
/// }).await?;
/// ```
///
/// # Note
///
/// This only catches panics in the direct poll path. Panics in spawned
/// sub-tasks are not caught.
pub async fn call_plugin_safely_async<F, Fut, T, E>(plugin_fn: F) -> Result<T, PluginError>
where
    F: FnOnce() -> Fut + UnwindSafe,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    // Wrapper future that catches panics during poll
    struct CatchUnwindFuture<F> {
        inner: F,
    }

    impl<F: Future> Future for CatchUnwindFuture<AssertUnwindSafe<F>> {
        type Output = Result<F::Output, Box<dyn Any + Send>>;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            // SAFETY: We're only projecting to the inner field
            let inner = unsafe { self.map_unchecked_mut(|s| &mut s.inner) };

            // Catch panics during poll
            match catch_unwind(AssertUnwindSafe(|| inner.poll(cx))) {
                Ok(Poll::Ready(output)) => Poll::Ready(Ok(output)),
                Ok(Poll::Pending) => Poll::Pending,
                Err(panic_info) => Poll::Ready(Err(panic_info)),
            }
        }
    }

    // First, safely create the future
    let future = match catch_unwind(plugin_fn) {
        Ok(fut) => fut,
        Err(panic_info) => {
            let msg = extract_panic_message(&panic_info);
            tracing::error!(message = %msg, "Plugin panicked during future creation");
            return Err(PluginError::Panic(msg));
        }
    };

    // Wrap in panic-catching future and await
    let catch_future = CatchUnwindFuture {
        inner: AssertUnwindSafe(future),
    };

    match catch_future.await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => Err(PluginError::InternalError(e.to_string())),
        Err(panic_info) => {
            let msg = extract_panic_message(&panic_info);
            tracing::error!(message = %msg, "Plugin panicked during async execution");
            Err(PluginError::Panic(msg))
        }
    }
}

/// Extract a human-readable message from panic info
///
/// Attempts to extract the panic message from the boxed Any type.
/// Handles common panic message types: &str, String, and falls back
/// to a generic message.
fn extract_panic_message(panic_info: &Box<dyn Any + Send>) -> String {
    if let Some(s) = panic_info.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = panic_info.downcast_ref::<String>() {
        s.clone()
    } else {
        "Unknown panic (non-string payload)".to_string()
    }
}

/// Wrapper type that makes a value UnwindSafe
///
/// Use this to wrap closures that capture mutable state but are known
/// to be safe to unwind through.
///
/// # Safety
///
/// The caller must ensure that unwinding through the wrapped value is safe.
/// This means:
/// - No invariants will be violated if the function panics
/// - No resources will be leaked
/// - No undefined behavior will result
pub struct SafeWrapper<T>(pub T);

impl<T> std::panic::UnwindSafe for SafeWrapper<T> {}
impl<T> std::panic::RefUnwindSafe for SafeWrapper<T> {}

impl<T> SafeWrapper<T> {
    /// Create a new SafeWrapper
    pub fn new(value: T) -> Self {
        Self(value)
    }

    /// Get the inner value
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> std::ops::Deref for SafeWrapper<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> std::ops::DerefMut for SafeWrapper<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_call_plugin_safely_success() {
        let result: Result<i32, PluginError> = call_plugin_safely(|| Ok::<_, std::io::Error>(42));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn test_call_plugin_safely_error() {
        let result: Result<i32, PluginError> = call_plugin_safely(|| {
            Err::<i32, _>(std::io::Error::new(std::io::ErrorKind::Other, "test error"))
        });
        assert!(result.is_err());
        match result {
            Err(PluginError::InternalError(msg)) => assert!(msg.contains("test error")),
            _ => panic!("Expected InternalError"),
        }
    }

    #[test]
    fn test_call_plugin_safely_panic_str() {
        let result: Result<i32, PluginError> = call_plugin_safely(|| {
            panic!("test panic message");
            #[allow(unreachable_code)]
            Ok::<_, std::io::Error>(42)
        });
        assert!(result.is_err());
        match result {
            Err(PluginError::Panic(msg)) => assert!(msg.contains("test panic message")),
            _ => panic!("Expected Panic error"),
        }
    }

    #[test]
    fn test_call_plugin_safely_panic_string() {
        let result: Result<i32, PluginError> = call_plugin_safely(|| {
            panic!("{}", "dynamic panic message".to_string());
            #[allow(unreachable_code)]
            Ok::<_, std::io::Error>(42)
        });
        assert!(result.is_err());
        match result {
            Err(PluginError::Panic(msg)) => assert!(msg.contains("dynamic panic message")),
            _ => panic!("Expected Panic error"),
        }
    }

    #[test]
    fn test_call_plugin_safely_value() {
        let result = call_plugin_safely_value(|| 42);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn test_call_plugin_safely_value_panic() {
        let result: Result<i32, PluginError> = call_plugin_safely_value(|| {
            panic!("value panic");
            #[allow(unreachable_code)]
            42
        });
        assert!(result.is_err());
    }

    #[test]
    fn test_safe_wrapper() {
        let mut counter = 0;
        let wrapper = SafeWrapper::new(&mut counter);
        let result: Result<(), PluginError> = call_plugin_safely(|| {
            // Can access wrapper in panic-safe context
            let _ = *wrapper;
            Ok::<_, std::io::Error>(())
        });
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_call_plugin_safely_async_success() {
        let result: Result<i32, PluginError> =
            call_plugin_safely_async(|| async { Ok::<_, std::io::Error>(42) }).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_call_plugin_safely_async_error() {
        let result: Result<i32, PluginError> = call_plugin_safely_async(|| async {
            Err::<i32, _>(std::io::Error::new(
                std::io::ErrorKind::Other,
                "async error",
            ))
        })
        .await;
        assert!(result.is_err());
    }
}

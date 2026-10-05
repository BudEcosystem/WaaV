//! Upload breakers: the shared state machine in [`crate::breaker`], by row, host and credential.

pub use crate::breaker::{
    Admission, Breaker as FileBreaker, BreakerConfig, BreakerRegistry, BreakerState,
};

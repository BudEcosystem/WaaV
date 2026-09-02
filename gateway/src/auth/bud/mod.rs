//! Bud control-plane integration: identity and credentials resolved from the same Redis
//! namespace budgateway reads, with no request-path I/O and no budapp involvement.
//!
//! Spec: `bud-runtime/specs/018-waav-voice-integration`.

pub mod hash;
pub mod snapshot;
pub mod types;

pub use hash::hash_api_key;
pub use snapshot::{BudAuth, BudSnapshot, Mutation};
pub use types::{AliasMap, AliasMetadata, AuthMetadata, UserProjects, VerifiedIdentity};

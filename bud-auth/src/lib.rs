//! Bud control-plane integration for WaaV.
//!
//! WaaV serves the whole audio plane, so it must authenticate the same callers budgateway does
//! — Bud project API keys and Keycloak JWTs — at the same scale, with the same revocation
//! semantics. This crate is a deliberate port of budgateway's auth design rather than a lighter
//! reimplementation, because the lighter designs all put a service call on a path that carries
//! ten thousand concurrent voice sessions.
//!
//! The invariants that matter are documented at their definitions, each with the failure it
//! prevents. See `bud-runtime/specs/018-waav-voice-integration` §5.3.
//!
//! It is a separate crate from `waav-gateway` on purpose: the auth plane is security-critical
//! and benefits from a test cycle measured in seconds rather than one that rebuilds a WebRTC
//! stack.

pub mod authz;
pub mod credentials;
pub mod guards;
pub mod hash;
pub mod hydrate;
pub mod jwt;
pub mod redis_store;
pub mod runtime;
pub mod snapshot;
pub mod store;
pub mod types;

pub use authz::{AuthzTier, Resolution};
pub use credentials::{CredentialDecryptor, CredentialError, VoiceEndpoint};
pub use guards::{Denied, EscalationPermit, KeyShape, MissGuardConfig, MissGuards};
pub use hash::hash_api_key;
pub use hydrate::{KeyEvent, hydrate_all};
pub use jwt::{JwksSource, JwtConfig, JwtVerifier, KeySet, Rejection};
pub use redis_store::{RedisStore, keyspace_patterns};
pub use runtime::{AuthFailure, BudPlane, Principal, PrincipalKind};
pub use snapshot::{BudAuth, BudSnapshot, Mutation};
pub use store::{ControlPlaneStore, MemoryStore, StoreError};
pub use types::{AliasMap, AliasMetadata, AuthMetadata, UserProjects, VerifiedIdentity};

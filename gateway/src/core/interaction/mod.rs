//! Interaction mode: the duplex voice agent that keeps the floor, acknowledges, delegates to
//! background tasks and relays their answers. One tokio actor per call (`actor`) owns four pure
//! machines (`duplex`, `floor`, `tasks`, `policy`) and performs their effects.
//!
//! Nothing here runs unless a `/ws` session asks for it and the server switch is on.

pub mod a2a_grpc;
pub mod actor;
pub mod analyser;
pub mod background;
pub mod bud_agent;
pub mod clips;
pub mod config;
pub mod controls;
pub mod duplex;
pub mod echo;
pub mod events;
pub mod executor;
pub mod floor;
pub mod front_model;
pub mod front_pool;
pub mod front_supervisor;
pub mod guard;
pub mod health;
pub mod hitl;
pub mod lifecycle;
pub mod persona;
pub mod policy;
pub mod provider;
pub mod regime;
pub mod remote;
pub mod rules;
pub mod speech;
pub mod store;
pub mod tasks;
pub mod tenant;
pub mod turn_record;
pub mod types;

pub use actor::Msg;
pub use config::{ConfigWarning, InteractionConfig, ServerInteraction};
pub use events::WireEvent;

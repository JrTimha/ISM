//! Cross-cutting infrastructure every domain builds on.
//!
//! `core` owns the layer contract itself — the [`Repository`] and [`Service`] traits, the
//! [`Database`] handle they are built from, the [`AppState`] that holds the wired services, and
//! the [`AppStateBuilder`] that constructs all of it. See `.claude/rules/architecture.md`.

mod app_state;
mod builder;
mod config;
pub mod cursor;
mod database;
pub mod errors;
mod extract;
pub mod model;
mod shutdown;
mod traits;

pub use app_state::*;
pub use builder::{AppStateBuilder, Bootstrap, Shutdown, StartupError, StartupResult};
pub use config::{ISMConfig, KafkaConfig, ObjectStorageConfig, RoomDbConfig, TokenIssuer};
pub use database::{Database, PgTransaction};
pub use extract::{ValidatedJson, ValidatedQuery};
pub use model::{ApiRequest, ApiResponse, DbRow, JsonColumn};
pub use shutdown::{ShutdownController, ShutdownSignal};
pub use traits::{Repository, Service};

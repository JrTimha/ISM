//! Business logic for the rooms domain.
//!
//! One file per service. Each is a [`Service`](crate::core::Service): a `Clone` struct holding its
//! dependencies, constructed once by [`AppStateBuilder`](crate::core::AppStateBuilder).

mod room;
mod share;
mod timeline;

pub use room::RoomService;
pub use share::{ActiveShareRow, InactiveShareRow, SharePhase, ShareService, ShareTarget, ShareTargetCursor, ShareTargetRef};
pub use timeline::TimelineService;

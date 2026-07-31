//! The wired application, and how a handler gets a piece of it.

use crate::core::ISMConfig;
use crate::messaging::{MessageService, NotificationService};
use crate::rooms::{RoomService, ShareService, TimelineService};
use crate::users::UserService;
use axum::extract::FromRef;
use std::sync::Arc;

/// Every service the API needs, built once by
/// [`AppStateBuilder`](crate::core::AppStateBuilder) and shared for the process's lifetime.
///
/// It holds **services only**. It used to hold repositories, the cache and the S3 client as well,
/// which made it a directory that any layer could look things up in — every service took an
/// `Arc<AppState>` and could therefore reach anything. Now infrastructure is injected into the
/// services that need it and stops there.
///
/// Handlers do not receive this type. They ask for the one service they use and axum's
/// [`FromRef`] hands it over; see the impls below.
#[derive(Clone)]
pub struct AppState {
    pub env: ISMConfig,
    pub room_service: RoomService,
    pub share_service: ShareService,
    pub timeline_service: TimelineService,
    pub message_service: MessageService,
    pub notification_service: NotificationService,
    pub user_service: UserService,
}

/// Lets a handler write `State<RoomService>` instead of `State<Arc<AppState>>`.
///
/// Each impl is a clone of one field, which is cheap: a service is a handful of `Arc` and pool
/// handles. The win is in the signatures — a handler now declares the single dependency it has,
/// so what it can touch is visible without reading its body.
macro_rules! service_from_state {
    ($($service:ty => $field:ident),+ $(,)?) => {
        $(
            impl FromRef<Arc<AppState>> for $service {
                fn from_ref(state: &Arc<AppState>) -> Self {
                    state.$field.clone()
                }
            }
        )+
    };
}

service_from_state! {
    RoomService => room_service,
    ShareService => share_service,
    TimelineService => timeline_service,
    MessageService => message_service,
    NotificationService => notification_service,
    UserService => user_service,
}

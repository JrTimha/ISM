//! The composition root: the one place that knows how to construct anything.
//!
//! Every dependency in ISM is built here, in dependency order, and handed to whatever needs it.
//! Nothing reaches for a global; nothing constructs a collaborator on the side. That is what makes
//! the service graph a DAG — a service can only be given something that already exists a few lines
//! above it, so a cycle is not expressible.

use crate::broadcast::BroadcastChannel;
use crate::cache::redis_cache::{Cache, NoOpCache, RedisCache};
use crate::core::{AppState, Database, ISMConfig, Repository, Service, ShutdownController};
use crate::kafka::PushNotificationProducer;
use crate::messaging::{ChatRepository, MessageService, NotificationService};
use crate::object_storage::ObjectStorage;
use crate::rooms::{RoomNotifier, RoomRepository, RoomService, ShareService, TimelineService};
use crate::users::{UserRepository, UserService};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::task::JoinHandle;
use tracing::{info, warn};

/// Why the process could not start.
///
/// Startup used to be a chain of `panic!`s and `.expect()`s spread across `AppState::new`,
/// `ObjectStorage::new`, `RedisCache::new` and `KafkaEventProducer::new`. An operator running the
/// container saw an unwind and a backtrace for what is almost always a misconfigured host or a
/// service that has not come up yet. Startup failures are expected, so they are a value.
#[derive(Debug, Error)]
pub enum StartupError {
    #[error("could not connect to PostgreSQL: {0}")]
    Database(#[from] sqlx::Error),

    #[error("could not connect to Redis: {0}")]
    Cache(#[from] redis::RedisError),

    #[error("could not reach the object storage: {0}")]
    ObjectStorage(String),

    #[error("could not create the Kafka producer: {0}")]
    Kafka(String),
}

/// Shorthand used by constructors that participate in startup.
pub type StartupResult<T> = Result<T, StartupError>;

/// A started application, split into the half that serves requests and the half that unwinds it.
///
/// The two are separate fields rather than one bundle because their lifetimes genuinely diverge:
/// [`Self::state`] is *moved* into the router and lives inside the server until it stops, while
/// [`Self::shutdown`] has to stay in the caller's hands to be usable afterwards. Bundling them
/// would force the caller to clone the state just to keep a handle on the teardown half.
///
/// ```ignore
/// let Bootstrap { state, shutdown } = AppStateBuilder::new(config).build().await?;
/// let app = init_router(state).await;   // moved, not cloned
/// // ... serve ...
/// shutdown.run().await;
/// ```
pub struct Bootstrap {
    /// The wired services, ready to be handed to the router.
    pub state: AppState,
    /// Everything that needs explicit teardown.
    pub shutdown: Shutdown,
}

/// The resources startup created that cannot simply be dropped.
///
/// Deliberately holds no [`AppState`]: teardown has nothing to do with serving requests, and
/// keeping the two apart is what lets the state be moved away while this stays behind. Fields are
/// private — only [`AppStateBuilder`] constructs one.
pub struct Shutdown {
    /// Handles of the long-running tasks the builder spawned.
    ///
    /// Kept rather than detached so the process can abort them. A global bus could never offer
    /// this — nobody owned the task it spawned.
    tasks: Vec<JoinHandle<()>>,
    /// The database handle, so the pool can be closed.
    ///
    /// Lives here rather than on [`AppState`] because it is a *lifecycle* concern, not something a
    /// request handler should be able to reach. Services hold their own clones for querying.
    database: Database,
    /// Fires the signal that tells live SSE/WebSocket connections to close themselves.
    ///
    /// The trigger half stays here; services only ever get the listen-only
    /// [`ShutdownSignal`](crate::core::ShutdownSignal).
    controller: ShutdownController,
}

impl Shutdown {
    /// Builds the future to hand to `axum::serve(..).with_graceful_shutdown(..)`.
    ///
    /// Awaits `trigger` — in practice the OS signal — and then fires the shutdown signal. Coupling
    /// the two here is the whole mechanism: axum stops accepting connections, and in the same
    /// moment every live stream is told to finish. Without the second half, axum would wait on
    /// those streams forever.
    ///
    /// The returned future owns a clone of the controller and captures nothing from `&self` (see
    /// the `use<F>` bound), so the borrow ends immediately and [`Self::run`] can still consume
    /// `self` afterwards.
    pub fn begin_when<F>(&self, trigger: F) -> impl Future<Output = ()> + Send + use<F>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let controller = self.controller.clone();
        async move {
            trigger.await;
            info!("Shutdown signal received, telling live connections to close.");
            controller.trigger();
        }
    }

    /// Resolves [`SHUTDOWN_GRACE`] after shutdown begins, and never before.
    ///
    /// Safe to race against the server for the whole life of the process: until the signal fires,
    /// this future simply stays pending. Once it does resolve, the caller stops *waiting* for the
    /// remaining connections — it cannot cancel them, because axum spawns each one as an
    /// independent task, but the runtime drops them when `main` returns.
    pub fn grace_deadline(&self) -> impl Future<Output = ()> + Send + use<> {
        let cancelled = self.controller.signal().cancelled();
        async move {
            cancelled.await;
            tokio::time::sleep(SHUTDOWN_GRACE).await;
        }
    }

    /// Stops the background tasks and closes the database pool.
    ///
    /// Order matters: the tasks are aborted first so nothing can check out a connection while the
    /// pool is closing.
    pub async fn run(self) {
        for task in self.tasks {
            task.abort();
        }

        // Bounded, because `close()` waits for every checked-out connection to come back. Nothing
        // should still hold one at this point, but a shutdown that hangs forever is worse than one
        // that gives up and lets the server's keepalive reap the rest.
        match tokio::time::timeout(SHUTDOWN_TIMEOUT, self.database.close()).await {
            Ok(()) => info!("Database pool closed."),
            Err(_) => warn!(timeout_secs = SHUTDOWN_TIMEOUT.as_secs(), "Database pool did not close in time, abandoning it"),
        }
    }
}

/// How long live connections get to close themselves before the process stops waiting for them.
///
/// Applies from the moment the shutdown signal fires. A stream that listens ends within
/// milliseconds; this bound only matters for one that does not.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(15);

/// How long shutdown waits for in-flight database connections to be returned.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

// The two bounds above run in sequence — connections first, then the pool — so the worst case is
// roughly their sum. Keep that total below the orchestrator's grace period (Kubernetes'
// `terminationGracePeriodSeconds`, Docker's `--stop-timeout`), or the process is SIGKILLed
// mid-teardown and the pool never gets closed at all.

/// Builds the [`AppState`].
///
/// The `with_*` methods are what make this a builder rather than a factory: a test can supply a
/// `NoOpCache` or a throwaway [`Database`] without a config file and without a live Redis, which
/// is only possible because nothing downstream reaches for a global.
///
/// ```ignore
/// let Bootstrap { state, tasks } = AppStateBuilder::new(config)
///     .with_cache(Arc::new(NoOpCache))
///     .build()
///     .await?;
/// ```
pub struct AppStateBuilder {
    config: ISMConfig,
    database: Option<Database>,
    cache: Option<Arc<dyn Cache>>,
    storage: Option<ObjectStorage>,
}

impl AppStateBuilder {
    pub fn new(config: ISMConfig) -> Self {
        Self {
            config,
            database: None,
            cache: None,
            storage: None,
        }
    }

    /// Uses an already-open database instead of connecting from config.
    ///
    /// Note that [`Shutdown::run`] closes whatever pool it ends up with, including one supplied
    /// here — so a test that injects a `#[sqlx::test]` pool should let the harness own the
    /// teardown and simply drop the [`Shutdown`] instead of running it.
    pub fn with_database(mut self, database: Database) -> Self {
        self.database = Some(database);
        self
    }

    /// Uses a specific cache implementation instead of choosing one from config.
    pub fn with_cache(mut self, cache: Arc<dyn Cache>) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Uses an already-connected object storage instead of connecting from config.
    pub fn with_storage(mut self, storage: ObjectStorage) -> Self {
        self.storage = Some(storage);
        self
    }

    /// Wires everything, in dependency order.
    pub async fn build(self) -> StartupResult<Bootstrap> {
        let config = self.config;
        // Currently empty: nothing the builder wires needs a background task of its own. Kept
        // because the *shutdown* contract lives here — anything spawned during wiring must be
        // pushed onto this, or `Shutdown::run` cannot abort it before the pool is closed.
        #[allow(unused_mut)]
        let mut tasks: Vec<JoinHandle<()>> = Vec::new();

        // Created first: services that own long-lived connections need the listen-only half, and
        // the trigger half goes to `Shutdown` at the end.
        let shutdown_controller = ShutdownController::new();

        // ── 1. Infrastructure ────────────────────────────────────────────────
        let database = match self.database {
            Some(database) => database,
            None => Database::connect(&config.room_db_config).await?,
        };
        info!("Established connection to the PostgreSQL database.");

        let cache: Arc<dyn Cache> = match (self.cache, &config.redis_cache_url) {
            (Some(cache), _) => cache,
            (None, Some(url)) => Arc::new(RedisCache::connect(url.clone()).await?),
            (None, None) => {
                info!("Redis is deactivated. Initializing NoOpCache...");
                Arc::new(NoOpCache)
            }
        };

        let storage = match self.storage {
            Some(storage) => storage,
            None => ObjectStorage::connect(&config.object_db_config).await?,
        };

        // ── 2. Event bus ─────────────────────────────────────────────────────
        let producer = PushNotificationProducer::connect(config.use_kafka, config.kafka_config.clone())?;
        let bus = Arc::new(BroadcastChannel::new(cache.clone(), producer));

        // ── 3. Repositories ──────────────────────────────────────────────────
        let rooms = RoomRepository::new(&database);
        let chats = ChatRepository::new(&database);
        let users = UserRepository::new(&database);

        // ── 4. Shared room-broadcasting component ────────────────────────────
        let notifier = RoomNotifier::new(bus.clone(), rooms.clone(), cache.clone());

        // ── 5. Services, in dependency order ─────────────────────────────────
        // Everything below depends only on what is already above it. `UserService` is last
        // because it is the only service that depends on another service.
        let room_service = RoomService::new(
            database.clone(),
            rooms.clone(),
            chats.clone(),
            users.clone(),
            notifier.clone(),
            storage,
            config.object_db_config.bucket_name.clone(),
        );
        let share_service = ShareService::new(rooms.clone());
        let timeline_service = TimelineService::new(rooms.clone(), chats.clone());
        let message_service = MessageService::new(database.clone(), rooms, chats, notifier);
        let notification_service = NotificationService::new(bus.clone(), cache, shutdown_controller.signal());
        let user_service = UserService::new(database.clone(), users, room_service.clone(), bus);

        for name in [
            RoomService::NAME,
            ShareService::NAME,
            TimelineService::NAME,
            MessageService::NAME,
            NotificationService::NAME,
            UserService::NAME,
        ] {
            info!(service = name, "Service wired");
        }

        Ok(Bootstrap {
            state: AppState {
                env: config,
                room_service,
                share_service,
                timeline_service,
                message_service,
                notification_service,
                user_service,
            },
            shutdown: Shutdown {
                tasks,
                database,
                controller: shutdown_controller,
            },
        })
    }
}

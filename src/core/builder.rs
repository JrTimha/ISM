//! The composition root: the one place that knows how to construct anything.
//!
//! Every dependency in ISM is built here, in dependency order, and handed to whatever needs it.
//! Nothing reaches for a global; nothing constructs a collaborator on the side. That is what makes
//! the service graph a DAG — a service can only be given something that already exists a few lines
//! above it, so a cycle is not expressible.

use crate::broadcast::BroadcastChannel;
use crate::cache::redis_cache::{Cache, NoOpCache, RedisCache};
use crate::cache::redis_subscriber::run_event_processor;
use crate::core::{AppState, Database, ISMConfig, Repository, Service};
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
}

impl Shutdown {
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
            Err(_) => warn!(
                timeout_secs = SHUTDOWN_TIMEOUT.as_secs(),
                "Database pool did not close in time, abandoning it"
            ),
        }
    }
}

/// How long shutdown waits for in-flight database connections to be returned.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

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
    ///
    /// A cache supplied this way brings no push-message receiver with it, so the Redis keyspace
    /// subscriber is not started — the bus still delivers to locally connected clients.
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
        let mut tasks: Vec<JoinHandle<()>> = Vec::new();

        // ── 1. Infrastructure ────────────────────────────────────────────────
        let database = match self.database {
            Some(database) => database,
            None => Database::connect(&config.room_db_config).await?,
        };
        info!("Established connection to the PostgreSQL database.");

        // The cache is built *without* starting its subscriber: that task needs the bus, and the
        // bus needs the cache. Returning the receiver instead of spawning inside the constructor
        // is what unties the knot — the ordering is resolved here, where both halves are in scope,
        // rather than by a lazily-initialised global.
        let (cache, push_messages): (Arc<dyn Cache>, _) = match (self.cache, &config.redis_cache_url) {
            (Some(cache), _) => (cache, None),
            (None, Some(url)) => {
                let redis = RedisCache::connect(url.clone()).await?;
                let connection = redis.cache.connection.clone();
                (Arc::new(redis.cache), Some((redis.push_messages, connection)))
            }
            (None, None) => {
                info!("Redis is deactivated. Initializing NoOpCache...");
                (Arc::new(NoOpCache), None)
            }
        };

        let storage = match self.storage {
            Some(storage) => storage,
            None => ObjectStorage::connect(&config.object_db_config).await?,
        };

        // ── 2. Event bus, then the task that feeds it ────────────────────────
        let producer = PushNotificationProducer::connect(config.use_kafka, config.kafka_config.clone())?;
        let bus = Arc::new(BroadcastChannel::new(cache.clone(), producer));

        if let Some((push_messages, connection)) = push_messages {
            // `bus.clone()` is an `Arc` clone moved into the task — the ordinary way to share with
            // a spawned future, and no less convenient than the global it replaces.
            tasks.push(tokio::spawn(run_event_processor(push_messages, connection, bus.clone())));
        }

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
        let notification_service = NotificationService::new(bus.clone(), cache);
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
            shutdown: Shutdown { tasks, database },
        })
    }
}

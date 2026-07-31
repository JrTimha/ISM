//! The single PostgreSQL handle every repository is built from.
//!
//! `Database` exists so that the connection pool has exactly one owner. Before it, every
//! repository carried its own `Pool<Postgres>` field and two of them handed it back out again
//! (`get_connection()`, `start_transaction()`) — which meant a service could reach the pool
//! through whichever repository happened to expose it, and transaction ownership was wherever the
//! caller found a `start_transaction`.
//!
//! Now: repositories *use* the database, services *own* transactions.
//!
//! ```ignore
//! // a transaction spanning two repositories is the service's business
//! let mut tx = self.db.begin().await?;
//! self.rooms.remove_user_from_room(&mut tx, &room_id, &user_id).await?;
//! self.chats.insert_message(&mut *tx, &message).await?;
//! tx.commit().await?;
//! ```
//!
//! # Sharing
//!
//! `Database` is [`Clone`] and **must** be shared by cloning, never by `Arc<Database>`. The
//! `Pool<Postgres>` inside it is already an `Arc` handle to one shared pool: cloning it hands out
//! another handle to the *same* connections, it does not open new ones. See
//! `.claude/rules/architecture.md`.

use crate::core::RoomDbConfig;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Pool, Postgres, Transaction};

/// Upper bound on connections held by the shared pool.
const MAX_CONNECTIONS: u32 = 20;

/// A transaction borrowed from the shared pool.
///
/// `'static` because [`Pool::begin`] hands out an owned `PoolConnection` — the transaction does not
/// borrow from the `Database` it came from and can therefore be moved freely.
pub type PgTransaction = Transaction<'static, Postgres>;

/// Shared handle to the PostgreSQL connection pool.
#[derive(Clone)]
pub struct Database {
    pool: Pool<Postgres>,
}

impl Database {
    /// Opens the shared connection pool.
    ///
    /// Returns an error instead of panicking so the composition root can report a usable startup
    /// failure — see [`crate::core::StartupError`].
    pub async fn connect(config: &RoomDbConfig) -> Result<Self, sqlx::Error> {
        let options = PgConnectOptions::new()
            .host(&config.db_host)
            .port(config.db_port)
            .database(&config.db_name)
            .username(&config.db_user)
            .password(&config.db_password);

        let pool = PgPoolOptions::new().max_connections(MAX_CONNECTIONS).connect_with(options).await?;

        Ok(Self { pool })
    }

    /// Wraps an already-open pool. Used by tests and by
    /// [`AppStateBuilder::with_database`](crate::core::AppStateBuilder::with_database).
    pub fn from_pool(pool: Pool<Postgres>) -> Self {
        Self { pool }
    }

    /// Begins a transaction.
    ///
    /// Only services call this. A transaction that spans several repositories is business logic,
    /// so it belongs to the layer that knows why the writes must be atomic.
    pub async fn begin(&self) -> Result<PgTransaction, sqlx::Error> {
        self.pool.begin().await
    }

    /// The pool as an [`sqlx::Executor`], for repository functions that are generic over their
    /// executor and are being called *outside* a transaction.
    ///
    /// Inside a transaction, pass `&mut *tx` instead — see `.docs/sqlx-executor-pattern.md`.
    pub fn pool(&self) -> &Pool<Postgres> {
        &self.pool
    }

    /// Closes the pool and its connections, waiting for checked-out ones to be returned.
    ///
    /// **Must be called on shutdown.** Rust has no async `Drop`, so dropping the last `Pool` handle
    /// only tears down the *client* side of each connection. PostgreSQL is not told the session
    /// ended and will hold the backend open until its TCP keepalive eventually notices — which can
    /// be minutes. A container restarted in a loop would accumulate abandoned backends and start
    /// hitting `max_connections`.
    ///
    /// `close()` also wakes any task blocked in `acquire()`, so nothing hangs waiting for a
    /// connection that is never coming.
    ///
    /// Safe to call through any clone: every handle points at the same shared pool.
    pub async fn close(&self) {
        self.pool.close().await;
    }
}

//! The two traits every layer of a domain implements.
//!
//! These are *convention* traits: they carry no `dyn` usage and no `async_trait`. Their job is to
//! make one construction shape compiler-enforced across the whole project, so a new domain cannot
//! quietly be wired differently from the existing ones.
//!
//! Deliberately **not** here: `trait UserService` / `trait UserRepository` abstractions over the
//! concrete types. In a language with reflection those buy testability; in Rust they cost an
//! `#[async_trait]` boxed future per call and a second copy of every signature, and a mocked
//! repository would only prove that the mock behaves like the mock — the SQL it replaces is
//! already type-checked against the live schema by the `sqlx` macros. Repositories are tested with
//! `#[sqlx::test]` against a real database; services are tested by constructing them with a
//! `NoOpCache` and a throwaway `BroadcastChannel`, which constructor injection makes possible.

use crate::core::Database;

/// Data access for one domain.
///
/// # Contract
///
/// - Holds the [`Database`] handle and **nothing else** — no cache, no event bus, no config, no
///   other repository. A repository that needs one of those is doing a service's job.
/// - Never begins a transaction. It *accepts* one: see `.docs/sqlx-executor-pattern.md` for when a
///   method takes `impl Executor<'_, Database = Postgres>` and when it takes `&mut PgConnection`.
/// - Returns `sqlx::Error`, not [`AppError`](crate::core::errors::AppError). Translating a database
///   failure into an HTTP-shaped error is a decision about the *use case*, so it belongs to the
///   service (`impl From<sqlx::Error> for AppError` makes that a plain `?`).
/// - Is `Clone` because every field it holds is cheap to clone. Never wrap one in an `Arc`.
///
/// ```ignore
/// #[derive(Clone)]
/// pub struct RoomRepository {
///     db: Database,
/// }
///
/// impl Repository for RoomRepository {
///     fn new(db: &Database) -> Self {
///         Self { db: db.clone() }
///     }
/// }
/// ```
pub trait Repository: Clone + Send + Sync + 'static {
    /// Builds the repository from the shared database handle.
    ///
    /// Takes `&Database` rather than `Database` so the composition root reads as
    /// `RoomRepository::new(&db)` for every repository, with no `.clone()` noise at the call site.
    fn new(db: &Database) -> Self;
}

/// Business logic for one domain.
///
/// # Contract
///
/// - Holds its dependencies as fields, injected through an explicit `new(...)`: repositories (its
///   own domain's and, where a plain query is needed, another domain's), `Arc<dyn Cache>`,
///   `Arc<BroadcastChannel>`, `ObjectStorage`, and a [`Database`] *only* if it owns transactions.
/// - Takes only the slice of configuration it actually uses — a bucket name, not `ISMConfig`.
/// - Never mentions an axum type. A service that knows about `State`, `Json` or `StatusCode` has
///   absorbed the handler layer.
/// - Owns authorization that requires a database read (room membership, block lists). The handler
///   above it validates *syntax*; the service validates *state*.
/// - Is `Clone`, so it can be handed to a handler through `FromRef`. Every field is cheap to
///   clone, which makes the whole struct cheap to clone. Never wrap one in an `Arc`.
///
/// # Depending on another service
///
/// Prefer holding another domain's *repository* over another domain's *service* — most
/// cross-domain needs are a single query, not a use case.
///
/// When a genuine service-to-service dependency exists, the graph must stay a DAG. Rust has no
/// garbage collector, so a cycle of `Arc`s is a permanent leak; here the cycle cannot even be
/// built, because the composition root constructs services in dependency order and a service can
/// only be handed something that already exists. The current graph has exactly one such edge:
/// `UserService` → `RoomService`.
pub trait Service: Clone + Send + Sync + 'static {
    /// Stable name for the startup wiring log and tracing spans.
    const NAME: &'static str;
}

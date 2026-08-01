# Layer Architecture Rules

The layer contract for every domain. New domains follow it without exception; if a rule here is
wrong for a case, change the rule rather than the one case.

```
Handler      extracts, validates syntax, calls one service          axum types live here and nowhere else
   ↓
Service      business logic; holds repositories + infrastructure    knows nothing about HTTP
   ↓
Repository   SQL; holds the Database handle and nothing else        returns sqlx::Error
```

Everything is wired once by `AppStateBuilder` (`core/builder.rs`), the only place in the project
that constructs anything.

## File layout

```
src/<domain>/
  mod.rs          public surface (re-exports the repository and services)
  routes.rs       Router<Arc<AppState>>
  handler.rs      axum handlers only
  repository.rs   one repository per domain
  service.rs      single-service domain (users)
  service/        multi-service domain (rooms, messaging), one file per service
  entity.rs       …Row structs (no serde) + …Json JSONB column payloads
  request.rs      …Request structs (Deserialize + Validate)
  response.rs     …Response structs (Serialize)
  model.rs        types shared across those boundaries: cursors, DB/wire enums, cache shapes
```

The four data-type files follow the same promote-to-a-directory rule as `service.rs` vs `service/`:
one file until it gets long, then a directory of the same name. Which type belongs in which file,
and why a row may not be a response, is `.claude/rules/model.md` — read it before adding a type.

## Repositories

```rust
#[derive(Clone)]
pub struct RoomRepository {
    db: Database,
}

impl Repository for RoomRepository {
    fn new(db: &Database) -> Self {
        Self { db: db.clone() }
    }
}
```

- Implement `core::Repository`. It is a convention trait — no `dyn`, no `async_trait`.
- Hold **`Database` and nothing else**. No cache, no event bus, no config, no other repository.
- **Never begin a transaction.** Accept one: `impl Executor<'_, Database = Postgres>` when the
  function is genuinely called both inside and outside a transaction, `&mut PgConnection` when it
  must always be part of a larger one. See `.docs/sqlx-executor-pattern.md`.
- **Never hand out the pool.** `get_connection()` and `start_transaction()` are gone; the only way
  to a transaction is `Database::begin()`, called by a service.
- Return `sqlx::Error`. Turning a database failure into an HTTP-shaped `AppError` is a decision
  about the use case, so the service makes it — `?` does the conversion.
- Tested with `#[sqlx::test]` against a real database. There is no repository trait to mock, and
  mocking one would only prove the mock behaves like the mock.

## Services

```rust
#[derive(Clone)]
pub struct RoomService {
    db: Database,              // only if it owns transactions
    rooms: RoomRepository,
    users: UserRepository,     // another domain's repository: fine
    notifier: RoomNotifier,
    storage: ObjectStorage,
    bucket: String,            // a config slice, never the whole ISMConfig
}

impl Service for RoomService {
    const NAME: &'static str = "RoomService";
}
```

- Implement `core::Service` and take dependencies through an explicit `new(...)`.
- **Own transactions.** A transaction spanning several repositories is business logic:
  `let mut tx = self.db.begin().await?;` then pass `&mut tx` / `&mut *tx` to the repositories.
- **Never mention an axum type.** A service that knows `State`, `Json` or `StatusCode` has absorbed
  the handler layer.
- **Own authorization that needs a database read** — room membership, block lists. See the split
  rule below.
- Take a config slice, not `ISMConfig`.

### The service dependency rule

Rust has no garbage collector: a cycle of `Arc`s never drops. Service dependencies must form a DAG,
and the builder's construction order is the mechanical proof — a service can only be handed
something already constructed above it, so a cycle will not compile.

- **Prefer another domain's repository over another domain's service.** Most cross-domain needs are
  one query, not a use case. `RoomService` needs block-list filtering, which is
  `UserRepository::find_blocked_relationships` — so it holds the repository, not `UserService`.
- Depend on another *service* only when you would otherwise duplicate a whole use case. The graph
  currently has exactly one such edge:

```
Tier 1:  RoomService, ShareService, TimelineService, MessageService, NotificationService
Tier 2:  UserService ──> RoomService     (blocking someone must tear down their shared 1-1 room)
```

## Handlers

```rust
pub async fn handle_leave_room(
    user: CurrentUser,
    State(rooms): State<RoomService>,
    Path(room_id): Path<Uuid>,
) -> AppResponse<()> {
    rooms.leave_room(user.subject, room_id).await?;
    Ok(())
}
```

- Take `State<XService>`, **never** `State<Arc<AppState>>`. axum resolves it through the `FromRef`
  impls in `core/app_state.rs`; a handler's signature is then an honest list of what it can reach.
- A handler that needs two services asks for two. If it needs three, the use case probably belongs
  in a service.
- **The split:** the handler validates *syntax* — `validator` derives, `decode_cursor`,
  `PageSize` clamping, multipart field extraction. Anything that requires reading the database,
  **including authorization**, is the service's. "Is this user in this room?" is not a handler
  question.
- See `.claude/rules/handlers.md` for extraction, return types and error variants.

## Sharing dependencies: `Arc` vs `Clone`

`Arc` is for **trait objects** and for types that are **not already cheap to clone**. Wrapping an
already-`Arc`'d type adds an indirection and buys nothing.

| Dependency | How to share it | Never |
|---|---|---|
| `Database` / `PgPool` | `#[derive(Clone)]`, clone freely — it *is* an `Arc` handle to one pool; cloning hands out another handle to the same connections | `Arc<Database>`, `Arc<PgPool>` |
| redis `ConnectionManager` | **clone it** — cheap, multiplexed over one connection, reconnects transparently, safe to share across tasks. Every `RedisCache` method does `self.connection.clone()` | `Arc<Mutex<Connection>>` — would funnel all Redis traffic through one lock and serialize what the manager is built to pipeline |
| `ObjectStorage` | `#[derive(Clone)]` — holds `Arc<MinioClient>` internally | `Arc<ObjectStorage>` |
| `PushNotificationProducer` | held by the bus; rdkafka's `FutureProducer` is itself `Clone` | `Arc<PushNotificationProducer>` |
| `dyn Cache` | `Arc<dyn Cache>` — real runtime polymorphism (Redis vs `NoOpCache`) | — |
| `BroadcastChannel` | `Arc<BroadcastChannel>` — holds a `RwLock`, not cheap to clone | a global |
| Repositories & services | `#[derive(Clone)]`; every field is cheap to clone, so the struct is too | `Arc<RoomService>` — `Arc<AppState>` already provides the one indirection |

## Background tasks

Constructors construct; the composition root spawns. `RedisCache::connect` opens a connection and
returns — it does not spawn, and neither does any other constructor.

There are currently **no** background tasks: `build()` still assembles a `Vec<JoinHandle<()>>`,
which is empty. Anything spawned during wiring goes onto it, with the dependency handed in as an
`Arc` clone moved to the spawn site rather than reached for through a global:

```rust
tasks.push(tokio::spawn({
    let bus = bus.clone();
    async move { /* … */ }
}));
```

The handles travel to `Shutdown`, which aborts them **before** closing the pool — so a task that is
spawned but not pushed can still be holding a connection while the pool drains. That is the whole
reason the vec stays: a global-spawned task offers no handle at all.

## Startup

`AppStateBuilder::build()` returns `Result<Bootstrap, StartupError>`. **No `panic!` or `.expect()`
on a startup path.** A missing bucket or an unreachable database is a misconfiguration, not a bug;
`main` prints it and exits non-zero.

The `with_database` / `with_cache` / `with_storage` overrides exist so a test can wire a partial
application without config files or a live Redis.

## Shutdown

**Always call `Shutdown::run()`**, on the failure path as well as the clean one.

`build()` returns `Bootstrap { state, shutdown }` — two fields, because their lifetimes diverge:

```rust
let Bootstrap { state, shutdown } = AppStateBuilder::new(config).build().await?;
let app = init_router(state).await;   // moved into the server for its whole life
// ... serve ...
shutdown.run().await;                 // the disjoint half that outlives it
```

`Shutdown` holds the spawned `JoinHandle`s and the `Database` — lifecycle concerns a request
handler has no business reaching — and deliberately holds **no** `AppState`. That separation is
what lets the state be *moved* into the router instead of cloned: Rust has no way to say "ownership
comes back later", so if teardown and serving share one struct, the caller is forced to clone. Two
disjoint owners need no clone at all. Prefer this over a cheap-clone workaround whenever two halves
of a value have genuinely different lifetimes.

### Long-lived connections must listen for the signal

**Axum cannot end a live connection for you.** Its graceful shutdown stops the accept loop and asks
each connection to finish its in-flight response — but an SSE body never finishes, and an upgraded
WebSocket is past the HTTP layer entirely. The serve future then waits on those connection tasks
forever, and there is no timeout to configure. This is not a bug to work around; it is what
graceful means.

So **any handler that holds a connection open past a single request/response must select on
`ShutdownSignal`**, or it holds the whole process open until it is killed — which also skips
`Shutdown::run()`, so the pool never closes either.

The signal is a `tokio::sync::watch` split into two capabilities: `ShutdownController` (fires it,
held only by `Shutdown`) and `ShutdownSignal` (observes it, cloned into services). A service that
owns long-lived work takes the listen-only half through its constructor, like any other dependency
— `NotificationService` does, and exposes it as `cancelled()` for both streaming handlers.

```rust
// SSE — one combinator ends the whole stream
let stream = replay_stream.chain(live_stream).take_until(notifications.cancelled());

// WebSocket — one more select arm; close with 1001 so the client reconnects
_ = &mut cancelled => {
    socket.send(Message::Close(Some(CloseFrame {
        code: close_code::AWAY,
        reason: Utf8Bytes::from_static("server shutting down"),
    }))).await.ok();
    break;
}
```

`main` races the server against `Shutdown::grace_deadline()`, which resolves `SHUTDOWN_GRACE` after
the signal fires. That is a **backstop, not the mechanism**: it cannot cancel the connections —
axum spawns each as its own task — it only stops waiting for them, leaving the runtime to drop them
when `main` returns. A handler that ignores the signal therefore degrades to "exits rudely" rather
than "never exits". `tests/graceful_shutdown.rs` pins both halves of this down.

`SHUTDOWN_GRACE` and `SHUTDOWN_TIMEOUT` run in sequence, so keep their sum under the orchestrator's
grace period or the process is SIGKILLed mid-teardown.

### Teardown order

`run()` aborts the background tasks first, then closes the pool, bounded by a timeout:

- **Closing the pool is not optional.** Rust has no async `Drop`, so dropping the last `Pool` handle
  tears down only the client side of each connection. PostgreSQL keeps the backend open until its
  TCP keepalive notices, which can take minutes — long enough for a restart loop to exhaust
  `max_connections`. `Database::close()` ends the sessions properly and wakes anything blocked in
  `acquire()`.
- **Tasks first**, so nothing can check out a connection while the pool is draining.
- **Bounded**, because `close()` waits for checked-out connections to come back; a shutdown that
  hangs forever is worse than one that gives up and lets the server reap the remainder.

Anything added later follows the same rule: if a resource needs an explicit async teardown, it
belongs in `Shutdown` and in `run()`, not in `AppState`.

# Authentication

Every route under `/api/v1` is protected by a Keycloak JWT. This document covers how the
middleware is wired, what handlers get, and the extension points that exist but are not currently
used.

The code lives in `src/auth/`. Submodules are private; everything a caller needs is re-exported
from `crate::auth` directly.

## How a request is authenticated

```
router::init_router
  └─ ServiceBuilder
       ├─ TraceLayer
       ├─ CorsLayer
       ├─ KeycloakAuthLayer   ← built in router::init_auth
       └─ DefaultBodyLimit
```

| Step | File | What happens |
|---|---|---|
| 1 | `extract.rs` | The raw JWT is pulled out of the `Authorization: Bearer …` header. |
| 2 | `instance.rs` | The realm's signing keys, fetched once at startup via OIDC discovery, are looked up by the token's `kid`. |
| 3 | `decode.rs` | Signature, algorithm, issuer, audience, expiry, `nbf`, token type and authorized party are checked against the `ValidationPolicy`. |
| 4 | `service.rs` | On success the `KeycloakToken` is inserted into the request extensions; on failure the request is answered with an error and never reaches the handler. |

The `ValidationPolicy` is built once at startup from the `[token_issuer]` config section, and a
misconfiguration (unknown algorithm, symmetric algorithm, empty audience list) panics at startup
rather than turning into a blanket 401 at runtime.

### Expiry and clock drift

Expiry is checked twice per request — once by `jsonwebtoken` as part of signature validation, once
explicitly by `KeycloakToken::assert_not_expired` — and the stricter of the two decides. Both read
the same `EXPIRY_LEEWAY_SECS` (`decode.rs`, currently **5 seconds**), so a token is accepted up to
five seconds past its `exp`. The same leeway applies to `nbf`.

That window exists to absorb clock drift between the Keycloak host and this one; it deliberately
replaces `jsonwebtoken`'s 60-second default. If you change it, change the constant — never one of
the two call sites, or the looser check silently becomes dead code. `security_tests.rs` pins the
boundary from both sides (expired by 2s must pass, expired by 10s must fail).

Beyond that window, correct expiry depends on both hosts having a synchronised clock. Neither
`chrono` nor any other date library affects this: every expiry check ultimately reads the host's
`CLOCK_REALTIME`, so NTP on both machines is the actual control.

### Key rotation

There is no refresh timer and no background task. When a token fails to verify in a way that
looks like a signing-key rotation, `decode_and_validate` re-runs OIDC discovery once and retries
within the same request. Rotation therefore costs added latency on exactly one request and is
never visible to a client. `KeycloakConfig::min_refresh_interval` (default 30s) rate-limits this,
so a flood of unverifiable tokens cannot turn into a request storm against Keycloak.

## What handlers get

Handlers take `CurrentUser`, which is a type alias for `KeycloakToken<AppRole>` — the whole
validated token, not just an id:

```rust
use crate::auth::CurrentUser;

pub async fn handle_get_friends(
    State(state): State<Arc<AppState>>,
    user: CurrentUser,
    Query(params): Query<RelationshipQueryParams>,
) -> AppResponse<Json<CursorResults<User>>> {
    let results = UserService::get_friends(state, &user.subject, /* … */).await?;
    Ok(Json(results))
}
```

| Field | Type | |
|---|---|---|
| `subject` | `Uuid` | the Keycloak user id — what nearly every handler passes to a service |
| `roles` | `Vec<KeycloakRole<AppRole>>` | realm and client roles |
| `extra.profile` | `Profile` | `preferred_username` |
| `extra.email` | `Email` | `email`, `email_verified` |
| `expires_at`, `issued_at` | `DateTime<Utc>` | |
| `issuer`, `audience`, `authorized_party`, `jwt_id` | | standard JWT claims |

`subject` is `Copy`, so `user.subject` inline is free. Bind a local only when the id has to
outlive the handler body — a `move` closure or a spawned task — so the capture is the id rather
than the whole token:

```rust
pub async fn websocket_server_events(
    websocket: WebSocketUpgrade,
    user: CurrentUser,
    Query(params): Query<StreamHandshakeParams>,
) -> impl IntoResponse {
    let user_id = user.subject;
    websocket.on_upgrade(move |socket| handle_socket(socket, user_id, params.last_seq))
}
```

Because `CurrentUser` pins the `<R>` generic to `AppRole`, no handler and no route ever spells the
role type out.

## Errors

`AuthError` covers everything that can go wrong. It is logged in full server-side and then
sanitised before it reaches the client — the response never says *which* check failed, only that
authentication failed. The wire format is the same `ErrorResponse` every other endpoint produces:

```json
{
  "timestamp": "…",
  "status": 401,
  "error": "Unauthorized",
  "message": "…",
  "path": "/api/v1/users/friends",
  "errorCode": "UNAUTHORIZED"
}
```

## Roles

`AppRole` (`auth/app_role.rs`) is the realm's role set and the concrete `Role` the whole
application is generic over:

```rust
pub enum AppRole {
    Admin,        // ADMIN
    User,         // USER
    LocalGuide,   // LOCAL_GUIDE
    Unknown(String),
}
```

`Unknown` is not an error case. Keycloak gives every account roles nobody here asked for —
`offline_access`, `uma_authorization`, `default-roles-<realm>`, plus the client roles of the
`account` client. Those land in `Unknown`, keep their original name for log messages, and never
match a check.

Role names are matched **case-sensitively** against the realm spelling, and `Display` emits that
same spelling — so `AppRole::from(role.to_string())` round-trips for every variant.

### Enforcing a role

**No route enforces one today.** Every authenticated user may call every endpoint. Two ways to
change that:

**Per handler**, with the macros from `role.rs`. They are `#[macro_export]`ed and therefore live
at the crate root, but resolve `ExpectRoles` through the `auth` facade:

```rust
use crate::auth::{AppRole, CurrentUser};
use crate::expect_role;

pub async fn admin_only(user: CurrentUser) -> Response {
    expect_role!(&user, AppRole::Admin);   // returns 403 early if absent
    StatusCode::OK.into_response()
}
```

Available: `expect_role!`, `expect_roles!`, `not_expect_role!`, `not_expect_roles!`. Each returns
early from the handler with an error response when the assertion fails — 403 with
`errorCode: "INSUFFICIENT_PERMISSIONS"`. The role that was missing is logged server-side but never
returned to the caller.

**Layer-wide**, when every route behind the layer needs the same role:

```rust
KeycloakAuthLayer::<AppRole>::builder()
    .instance(instance)
    .required_roles(vec![AppRole::Admin])
    .build()
```

Be careful with this one: it applies to every protected route at once, so requiring a role that
not every account actually holds locks those users out of the whole API.

### Adding a role

Add the variant, its realm spelling in `as_str`, and the `From<String>` arm — the tests in
`app_role.rs` cover the mapping and the round-trip. Nothing else changes: `CurrentUser` and
`init_auth` already carry the type.

## Extension points

The two sections below describe capabilities the middleware has but ISM does not currently use.

### Passthrough modes

`KeycloakAuthLayer::passthrough_mode` decides what happens to a request that fails validation:

- `PassthroughMode::Block` — answer immediately with an error response. This is the default and
  what ISM uses. On success the handler finds a `KeycloakToken` in the request extensions.
- `PassthroughMode::Pass` — always call the handler, and store a `KeycloakAuthStatus`
  (`Success(token)` or `Failure(error)`) in the extensions instead. Use this when a route needs
  fine-grained handling of the failure, or when a deeper layer might still authenticate the user.

Note that `CurrentUser` assumes `Block`: under `Pass` there is no `KeycloakToken` extension to
read, and the extractor rejects with 401.

### Custom token extractors

By default the layer looks for `Authorization: Bearer <token>`. `token_extractors` takes a list of
`TokenExtractor` implementations, tried in order; the first one to yield a token wins and the rest
are skipped. An empty list fails closed — no extractor produces a token, so every request is
rejected with 401.

Two implementations ship in `extract.rs`:

- `AuthHeaderTokenExtractor` — the `Authorization` header. The default.
- `QueryParamTokenExtractor` — a query parameter, `?token=…` by default.

```rust
KeycloakAuthLayer::<AppRole>::builder()
    .instance(instance)
    .validation_policy(policy)
    .token_extractors(vec![
        Arc::new(AuthHeaderTokenExtractor::default()) as Arc<dyn TokenExtractor>,
        Arc::new(QueryParamTokenExtractor::default()),
        Arc::new(QueryParamTokenExtractor::extracting_key("jwt")),
    ])
    .build()
```

**Security note on query parameters**: URLs end up in access logs, proxy logs and `Referer`
headers, so a token passed this way is far more likely to be recorded somewhere than one in a
header. The reason it exists at all is `/api/wss`: the browser `WebSocket` API cannot set request
headers, so a browser client has no way to send `Authorization` on the upgrade request. If that
becomes necessary, prefer a short-lived single-use ticket over the access token itself.

## Tests

`src/auth/security_tests.rs` runs the validation path against deliberately malformed and
malicious tokens — `alg=none`, RS256→HS256 confusion, tampered payloads, foreign issuers and
audiences, expired and not-yet-valid tokens, ID and refresh tokens replayed as access tokens,
non-UUID subjects. It signs with an in-file test key and touches no network.

**When you add a validation rule, add the attack it defeats.** A rule with no test that failed
before it existed is a rule nobody can prove still works.

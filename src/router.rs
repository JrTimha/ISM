use crate::auth::PassthroughMode;
use crate::auth::decode::ValidationPolicy;
use crate::auth::instance::{KeycloakAuthInstance, KeycloakConfig};
use crate::auth::layer::KeycloakAuthLayer;
use crate::core::{AppState, TokenIssuer};
use crate::messaging::routes::create_messaging_routes;
use crate::rooms::routes::create_room_routes;
use crate::users::routes::create_user_routes;
use axum::Router;
use axum::body::to_bytes;
use axum::extract::DefaultBodyLimit;
use axum::extract::{MatchedPath, Request};
use axum::http::Uri;
use axum::http::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use http::header::{CONNECTION, CONTENT_LENGTH, ORIGIN};
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;
use tower_http::trace::{DefaultOnFailure, TraceLayer};
use tracing::Level;
use url::Url;

/**
 * Initializing the api routes.
 */
pub async fn init_router(app_state: AppState) -> Router {
    let origin = app_state.env.cors_origin.clone();
    let cors = CorsLayer::new()
        .allow_origin(origin.parse::<HeaderValue>().expect("Invalid CORS Origin"))
        .allow_headers([
            AUTHORIZATION,
            ACCEPT,
            CONTENT_TYPE,
            CONTENT_LENGTH,
            CONNECTION,
            ORIGIN,
        ])
        .allow_credentials(true)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS, Method::DELETE]);

    let public_routing = Router::new()
        .route("/", get(|| async { "Hello, world! I'm your new ISM. 🤗" }))
        .route(
            "/health",
            get(|| async { (StatusCode::OK, "Healthy").into_response() }),
        );

    let protected_routing = Router::new()
        .nest(
            "/api/v1", //add new routes here, the /api prefix is applied once via nest
            Router::new()
                .merge(create_room_routes())
                .merge(create_user_routes())
                .merge(create_messaging_routes()),
        )
        //layering bottom to top middleware
        .layer(
            ServiceBuilder::new() //layering top to bottom middleware
                .layer(http_trace_layer()) //1
                .layer(cors) //2
                .layer(init_auth(app_state.env.token_issuer.clone())) //3..
                .layer(DefaultBodyLimit::max(5 * 1024 * 1024)), //max 5mb files
        )
        .layer(axum::middleware::from_fn(inject_request_path))
        .with_state(Arc::new(app_state));

    public_routing.merge(protected_routing)
}

/// HTTP request tracing.
///
/// The defaults of `TraceLayer::new_for_http()` put both the span and the response event at DEBUG,
/// which means nothing at all is emitted at the default INFO level — and application errors then
/// arrive with no request context attached. Spans are built explicitly here instead.
fn http_trace_layer() -> TraceLayer<
    tower_http::classify::SharedClassifier<tower_http::classify::ServerErrorsAsFailures>,
    impl Fn(&Request) -> tracing::Span + Clone,
> {
    TraceLayer::new_for_http()
        .make_span_with(|request: &Request| {
            // The matched route, not the raw URI: path parameters would give the span unbounded
            // cardinality.
            let path = request
                .extensions()
                .get::<MatchedPath>()
                .map(|mp| mp.as_str())
                .unwrap_or_else(|| request.uri().path());

            tracing::info_span!(
                "HTTP_REQUEST",
                method = %request.method(),
                path = %path,
            )
        })
        .on_failure(DefaultOnFailure::new().level(Level::ERROR))
}

async fn inject_request_path(
    matched_path: Option<MatchedPath>,
    uri: Uri,
    req: Request,
    next: Next,
) -> Response {
    let path = matched_path
        .map(|mp| mp.as_str().to_owned())
        .unwrap_or_else(|| uri.path().to_owned());

    let response = next.run(req).await;

    if !response.status().is_client_error() && !response.status().is_server_error() {
        return response;
    }

    let (mut parts, body) = response.into_parts();
    let bytes = match to_bytes(body, 64 * 1024).await {
        Ok(b) => b,
        Err(_) => return Response::from_parts(parts, axum::body::Body::empty()),
    };

    if let Ok(mut json) = serde_json::from_slice::<serde_json::Value>(&bytes) {
        if let Some(obj) = json.as_object_mut() {
            obj.insert("path".to_owned(), serde_json::json!(path));
        }
        if let Ok(new_body) = serde_json::to_vec(&json) {
            parts.headers.remove(CONTENT_LENGTH);
            return Response::from_parts(parts, axum::body::Body::from(new_body));
        }
    }

    Response::from_parts(parts, axum::body::Body::from(bytes))
}

fn init_auth(config: TokenIssuer) -> KeycloakAuthLayer<String> {
    let server = Url::parse(&config.iss_host).expect("Invalid Keycloak Host");

    if server.scheme() != "https" {
        tracing::warn!(
            keycloak_host = %server,
            "Keycloak is configured over plaintext HTTP. Tokens and the JWKS are exchanged in the \
             clear — use HTTPS outside of local development."
        );
    }

    let policy = ValidationPolicy::new(
        config.expected_audiences,
        config.expected_azp,
        &config.allowed_algorithms,
    )
    .unwrap_or_else(|err| panic!("Invalid [token_issuer] configuration: {err}"));

    if policy.expected_audiences.iter().any(|it| it == "account") && policy.expected_azp.is_empty()
    {
        tracing::warn!(
            "Accepting the 'account' audience with no 'expected_azp' restriction: Keycloak adds \
             this audience to every access token of every client in the realm, so any client in \
             the realm is accepted. Configure an audience mapper and set expected_audiences / \
             expected_azp."
        );
    }

    let keycloak_auth_instance = KeycloakAuthInstance::new(
        KeycloakConfig::builder()
            .server(server)
            .realm(config.iss_realm)
            .min_refresh_interval(Duration::from_secs(config.jwks_min_refresh_interval_secs))
            .build(),
    );

    KeycloakAuthLayer::<String>::builder()
        .instance(keycloak_auth_instance)
        // Nothing reads the raw claim map; persisting it clones the whole map per request.
        .persist_raw_claims(false)
        .passthrough_mode(PassthroughMode::Block)
        .validation_policy(policy)
        .build()
}

//! The tracing span every log line from a request hangs off.

use axum::extract::{MatchedPath, Request};
use tower_http::classify::{ServerErrorsAsFailures, SharedClassifier};
use tower_http::trace::{DefaultOnFailure, TraceLayer};
use tracing::Level;

/// HTTP request tracing.
///
/// The defaults of `TraceLayer::new_for_http()` put both the span and the response event at DEBUG,
/// which means nothing at all is emitted at the default INFO level — and application errors then
/// arrive with no request context attached. Spans are built explicitly here instead.
pub fn http_trace_layer() -> TraceLayer<
    SharedClassifier<ServerErrorsAsFailures>,
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
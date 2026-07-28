//! A single shared async operation whose latest result can be read while it re-runs.
//!
//! Used for exactly one thing: holding the OIDC discovery result inside a
//! `KeycloakAuthInstance` (see `instance.rs`), so that requests keep reading the cached keys
//! while a refresh is in flight and concurrent callers coalesce onto one run.
//!
//! The action reports failure as `None` rather than storing it, so an unsuccessful run leaves the
//! last good value in place. For the one caller that matters this is the difference between a
//! failed JWKS refresh costing nothing and it taking authentication down until the next successful
//! discovery — there is no timer, so "the next one" may be a long way off.

use educe::Educe;
use futures::Future;
use std::sync::atomic::Ordering;
use std::{
    fmt::Debug,
    option::Option,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize},
    },
};
use tokio::{
    sync::Notify,
    sync::{RwLock, futures::Notified},
    task::JoinHandle,
};

pub trait ActionInput: Debug + Clone + Send + Sync + 'static {}
pub trait ActionOutput: Debug + Send + Sync + 'static {}

impl<T> ActionInput for T where T: Debug + Clone + Send + Sync + 'static {}
impl<T> ActionOutput for T where T: Debug + Send + Sync + 'static {}

#[derive(Educe)]
#[educe(Debug)]
pub struct Action<I: ActionInput, O: ActionOutput> {
    /// The current argument that was dispatched to the `async` function.
    /// `Some` while we are waiting for it to resolve, `None` if it has resolved.
    input: Arc<RwLock<Option<I>>>,

    /// Yields `None` when the run failed, which `dispatch` treats as "keep what we have".
    #[educe(Debug(ignore))]
    #[allow(clippy::complexity)]
    action_fn:
        Arc<dyn Fn(&I) -> Pin<Box<dyn Future<Output = Option<O>> + Send + Sync>> + Send + Sync>,

    /// Might be Some if there still is an ongoing operation.
    pending: Arc<AtomicBool>,

    notify: Arc<Notify>,

    /// The most recent *successful* return value of the `async` function.
    ///
    /// A run that yields `None` leaves this untouched — see `dispatch`.
    value: Arc<RwLock<Option<O>>>,

    /// How many times the action has resolved, successfully or not.
    /// Version 0 indicates that no run has completed yet.
    version: Arc<AtomicUsize>,
}

impl<I: ActionInput, O: ActionOutput> Action<I, O> {
    pub fn new<F, Fu>(action_fn: F) -> Self
    where
        F: Fn(&I) -> Fu + Send + Sync + 'static,
        Fu: Future<Output = Option<O>> + Send + Sync + 'static,
    {
        let action_fn = Arc::new(move |input: &I| {
            let fut = action_fn(input);
            Box::pin(fut) as Pin<Box<dyn Future<Output = Option<O>> + Send + Sync>>
        });

        Self {
            input: Arc::new(RwLock::new(None)),
            action_fn,
            pending: Arc::new(AtomicBool::new(false)),
            notify: Arc::new(Notify::new()),
            value: Arc::new(RwLock::new(None)),
            version: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Await the next completion of this action.
    /// Useful if the action is already pending, and you are interested in its upcoming value.
    pub fn notified(&self) -> Notified<'_> {
        self.notify.notified()
    }

    pub fn is_pending(&self) -> bool {
        self.pending.load(Ordering::Acquire)
    }

    pub async fn input(&self) -> tokio::sync::RwLockReadGuard<'_, Option<I>> {
        self.input.read().await
    }

    pub async fn value(&self) -> tokio::sync::RwLockReadGuard<'_, Option<O>> {
        self.value.read().await
    }

    pub fn version(&self) -> usize {
        self.version.load(Ordering::Acquire)
    }

    /// Installs a value without running the action, as the result of a run performed by the caller.
    ///
    /// Exists so the initial OIDC discovery can be awaited directly — and its failure turned into a
    /// startup abort — rather than being dispatched into a task whose outcome nobody can observe.
    pub async fn seed(&self, value: O) {
        *self.value.write().await = Some(value);
        self.version.fetch_add(1, Ordering::Release);
    }

    pub fn dispatch(&self, action_input: I) -> JoinHandle<()> {
        let fut = (self.action_fn)(&action_input);
        let input = self.input.clone();
        let version = self.version.clone();
        let pending = self.pending.clone();
        let notify = self.notify.clone();
        let value = self.value.clone();

        // Mark pending *before* spawning. Setting it inside the task leaves a window in which the
        // action is dispatched but not yet observably pending, so concurrent callers checking
        // `is_pending()` would each dispatch their own duplicate run.
        pending.store(true, Ordering::Release);

        tokio::spawn(async move {
            *input.write().await = Some(action_input.clone());
            // A failed run must not evict the value a successful one left behind: the stored value
            // is what every request reads, and replacing it with nothing turns one unlucky refresh
            // into a total outage. The run is still counted below so waiters wake either way.
            if let Some(new_value) = fut.await {
                *value.write().await = Some(new_value);
            }
            version.fetch_add(1, Ordering::Release);
            *input.write().await = None;
            pending.store(false, Ordering::Release);
            notify.notify_waiters();
        })
    }
}

#[cfg(test)]
#[allow(unused)]
mod test {
    use assertr::prelude::*;

    use super::{Action, ActionInput, ActionOutput};

    pub trait ActionAssertions<I: ActionInput, O: ActionOutput> {
        fn has_version(self, expected: usize) -> Self;
        #[allow(clippy::wrong_self_convention)]
        fn is_pending(self, expected: bool) -> Self;
        async fn has_input(self, expected: Option<&I>) -> Self
        where
            I: PartialEq;
        async fn has_value(self, expected: Option<&O>) -> Self
        where
            O: PartialEq;
    }

    impl<I: ActionInput, O: ActionOutput, M: Mode> ActionAssertions<I, O>
        for AssertThat<'_, Action<I, O>, M>
    {
        #[track_caller]
        fn has_version(self, expected: usize) -> Self {
            self.derive(|it| it.version()).is_equal_to(expected);
            self
        }

        #[track_caller]
        fn is_pending(self, expected: bool) -> Self {
            self.derive(|it| it.is_pending()).is_equal_to(expected);
            self
        }

        async fn has_input(self, expected: Option<&I>) -> Self
        where
            I: PartialEq,
        {
            {
                let input = self.actual().input().await;
                let input_ref = input.as_ref();
                self.derive(move |_it| input_ref).is_equal_to(expected);
            }
            self
        }

        async fn has_value(self, expected: Option<&O>) -> Self
        where
            O: PartialEq,
        {
            {
                let value = self.actual().value().await;
                let value_ref = value.as_ref();
                self.derive(move |_it| value_ref).is_equal_to(expected);
            }
            self
        }
    }
}

//! Per-request analytics context: GPC/DNT and consent, framework-agnostic.
//!
//! A web framework's middleware wraps each request in
//! [`RequestContext::scope`] (an async handler) or holds a
//! [`RequestContext::enter`] guard (sync code), passing what it read from the
//! headers (`Sec-GPC: 1`, `DNT: 1`) and, for `ANALYTICS_CONSENT=opt-in`,
//! whether the visitor consented. `track()` on the same thread inside that
//! scope consults it.
//!
//! The state is a thread-local set around every `poll` of the wrapped
//! future, so it is right on any executor and follows the future across
//! worker threads, and never leaks between concurrent requests. It does not
//! follow into tasks the handler spawns (`tokio::spawn`) or threads it
//! starts: wrap those futures in their own `scope` (or call `enter` there).

use std::cell::Cell;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll};

use pin_project_lite::pin_project;

/// What the current request says about tracking.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RequestContext {
    /// `Sec-GPC: 1` or `DNT: 1` was sent.
    pub gpc: bool,
    /// The visitor opted in (only consulted when `ANALYTICS_CONSENT=opt-in`).
    pub consent: bool,
}

thread_local! {
    static CURRENT: Cell<Option<RequestContext>> = const { Cell::new(None) };
}

/// The active context, or `None` outside any scope.
#[must_use]
pub fn current() -> Option<RequestContext> {
    CURRENT.get()
}

/// Restores the previous context when dropped. Not `Send`: it belongs to the
/// thread that entered.
#[must_use = "the context ends when the guard is dropped"]
pub struct ContextGuard {
    previous: Option<RequestContext>,
    _not_send: PhantomData<*const ()>,
}

impl Drop for ContextGuard {
    fn drop(&mut self) {
        CURRENT.set(self.previous);
    }
}

impl RequestContext {
    /// Make `self` the context of the current thread until the guard drops.
    pub fn enter(self) -> ContextGuard {
        ContextGuard {
            previous: CURRENT.replace(Some(self)),
            _not_send: PhantomData,
        }
    }

    /// Run `future` with `self` as the context on every poll.
    pub fn scope<F: Future>(self, future: F) -> Scoped<F> {
        Scoped {
            context: self,
            future,
        }
    }
}

pin_project! {
    /// The future [`RequestContext::scope`] returns.
    pub struct Scoped<F> {
        context: RequestContext,
        #[pin]
        future: F,
    }
}

impl<F: Future> Future for Scoped<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let _guard = this.context.enter();
        this.future.poll(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guards_nest_and_restore() {
        assert_eq!(current(), None);
        {
            let _a = RequestContext {
                gpc: true,
                consent: false,
            }
            .enter();
            assert_eq!(current().map(|c| c.gpc), Some(true));
            {
                let _b = RequestContext {
                    gpc: false,
                    consent: true,
                }
                .enter();
                assert_eq!(current().map(|c| c.consent), Some(true));
            }
            assert_eq!(current().map(|c| c.gpc), Some(true));
        }
        assert_eq!(current(), None);
    }

    #[tokio::test]
    async fn a_scope_sets_the_context_on_every_poll_and_only_there() {
        let seen = RequestContext {
            gpc: true,
            consent: true,
        }
        .scope(async {
            tokio::task::yield_now().await;
            current()
        })
        .await;
        assert_eq!(
            seen,
            Some(RequestContext {
                gpc: true,
                consent: true
            })
        );
        assert_eq!(current(), None);
        let spawned = RequestContext {
            gpc: true,
            consent: false,
        }
        .scope(async { tokio::spawn(async { current() }).await.unwrap() })
        .await;
        assert_eq!(spawned, None, "the context does not follow a spawned task");
    }
}

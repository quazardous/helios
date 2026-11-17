//! Abandoning a render that is no longer wanted.
//!
//! Whoever starts a render (a warm-up, a preview) may hand it a [`Token`]
//! with [`scope`]; the heavy modules read the current one with
//! [`current`] and check it between bands of work, giving up with
//! [`CANCELLED`] once it is set. Outside a scope the token is never set,
//! so nothing changes for a render nobody cancels.
//!
//! A computation several askers share runs under a [`Token::shared`]:
//! each asker [`Token::join`]s its own token, and the shared one counts as
//! cancelled only once every asker cancelled, so one asker giving up never
//! takes the result from the others.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// The error a module returns when it gave up on a cancelled render.
pub const CANCELLED: &str = "cancelled";

/// A flag shared between whoever may cancel a render and the work in it.
#[derive(Clone, Debug, Default)]
pub struct Token(Arc<Inner>);

#[derive(Debug, Default)]
struct Inner {
    flag: AtomicBool,
    /// For a shared token: the tokens of its askers.
    askers: Option<Mutex<Vec<Token>>>,
}

impl Token {
    pub fn new() -> Self {
        Self::default()
    }

    /// A token for work several askers share: cancelled once every asker
    /// that [`join`](Self::join)ed cancelled (or by [`cancel`](Self::cancel)
    /// itself). With no asker yet it is not cancelled.
    pub fn shared() -> Self {
        Token(Arc::new(Inner {
            flag: AtomicBool::new(false),
            askers: Some(Mutex::new(Vec::new())),
        }))
    }

    /// Adds an asker to a shared token: the work goes on while it has not
    /// cancelled. Does nothing on a token that is not shared.
    pub fn join(&self, asker: Token) {
        if let Some(askers) = &self.0.askers {
            let mut askers = askers.lock().unwrap_or_else(PoisonError::into_inner);
            if !askers.iter().any(|t| Arc::ptr_eq(&t.0, &asker.0)) {
                askers.push(asker);
            }
        }
    }

    /// Asks the work holding this token to stop at its next check.
    pub fn cancel(&self) {
        self.0.flag.store(true, Ordering::Relaxed);
    }

    /// Whether the work should stop: one relaxed atomic load, and for a
    /// shared token a look at its askers.
    pub fn is_cancelled(&self) -> bool {
        if self.0.flag.load(Ordering::Relaxed) {
            return true;
        }
        match &self.0.askers {
            None => false,
            Some(askers) => {
                let askers = askers.lock().unwrap_or_else(PoisonError::into_inner);
                !askers.is_empty() && askers.iter().all(Token::is_cancelled)
            }
        }
    }
}

thread_local! {
    static CURRENT: RefCell<Option<Token>> = const { RefCell::new(None) };
}

/// Runs `f` with `token` as this thread's current token, restoring the
/// previous one after.
pub fn scope<R>(token: &Token, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Token>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CURRENT.with(|c| *c.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(CURRENT.with(|c| c.borrow_mut().replace(token.clone())));
    f()
}

/// This thread's current token: the one of the innermost [`scope`], or a
/// token nobody can set outside any. Take it before handing work to
/// other threads.
pub fn current() -> Token {
    CURRENT.with(|c| c.borrow().clone()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scope_hands_its_token_and_restores_the_previous_one() {
        assert!(!current().is_cancelled());
        let outer = Token::new();
        outer.cancel();
        scope(&outer, || {
            assert!(current().is_cancelled());
            scope(&Token::new(), || assert!(!current().is_cancelled()));
            assert!(current().is_cancelled());
        });
        assert!(!current().is_cancelled());
    }

    #[test]
    fn a_shared_token_is_cancelled_once_every_asker_cancelled() {
        let shared = Token::shared();
        assert!(!shared.is_cancelled(), "no asker yet");
        let (one, two) = (Token::new(), Token::new());
        shared.join(one.clone());
        shared.join(two.clone());
        shared.join(one.clone());
        one.cancel();
        assert!(!shared.is_cancelled(), "the second asker still wants it");
        two.cancel();
        assert!(shared.is_cancelled());
        // A shared token nested in another counts as one asker.
        let outer = Token::shared();
        outer.join(shared);
        assert!(outer.is_cancelled());
        outer.join(Token::new());
        assert!(!outer.is_cancelled());
    }
}

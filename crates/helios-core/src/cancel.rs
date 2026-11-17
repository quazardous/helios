//! Abandoning a render that is no longer wanted.
//!
//! Whoever starts a render (a warm-up, a preview) may hand it a [`Token`]
//! with [`scope`]; the heavy modules read the current one with
//! [`current`] and check it between bands of work, giving up with
//! [`CANCELLED`] once it is set. Outside a scope the token is never set,
//! so nothing changes for a render nobody cancels.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The error a module returns when it gave up on a cancelled render.
pub const CANCELLED: &str = "cancelled";

/// A flag shared between whoever may cancel a render and the work in it.
#[derive(Clone, Debug, Default)]
pub struct Token(Arc<AtomicBool>);

impl Token {
    pub fn new() -> Self {
        Self::default()
    }

    /// Asks the work holding this token to stop at its next check.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Whether the work should stop: one relaxed atomic load.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
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
}

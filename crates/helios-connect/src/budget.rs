//! One rate budget per account of a connector's service, shared by every
//! request Helios makes there: an import's downloads and the import
//! window's glances (thumbnails) draw from the same [`Budget`], so running
//! both at once never asks more than the connector declared ([`Limits`]).
//!
//! - **Rate**: requests a second, a minute, a day, counted over a sliding
//!   window; a wait the service asked for ([`Budget::hold`]) holds every
//!   request until it is past.
//! - **Parallel**: requests under way at once ([`Limits::parallel`]); a
//!   [`Permit`] keeps its place until dropped.
//! - **Downloads first**: while a download waits ([`Budget::queue`]), no
//!   thumbnail is given a place.
//!
//! The clock is the caller's (`now`): tests count on a fake one.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use crate::{Limits, SourceRef};

const SECOND: Duration = Duration::from_secs(1);
const MINUTE: Duration = Duration::from_mins(1);
const DAY: Duration = Duration::from_hours(24);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What a request is for: downloads go before thumbnails.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Use {
    Download,
    Thumbnail,
}

/// The requests one account may make, held to its connector's [`Limits`].
#[derive(Debug)]
pub struct Budget {
    limits: Limits,
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    /// When each request of the longest window counted went.
    at: VecDeque<Instant>,
    /// No request before this: the service asked to wait.
    hold: Option<Instant>,
    /// Requests under way.
    running: u32,
    /// Downloads waiting for a place.
    downloads: u32,
    /// The daily quota started again then ([`Budget::new_day`]): the
    /// requests before it no longer count for the day.
    day_from: Option<Instant>,
}

/// A place in the budget: the request it was given for is under way until
/// it is dropped.
#[derive(Debug)]
pub struct Permit(Arc<Budget>);

impl Drop for Permit {
    fn drop(&mut self) {
        let mut state = lock(&self.0.state);
        state.running = state.running.saturating_sub(1);
    }
}

/// A request waiting for a place ([`Budget::queue`]): while a download's
/// ticket lives, thumbnails wait behind it.
#[derive(Debug)]
pub struct Ticket {
    budget: Arc<Budget>,
    use_: Use,
}

impl Ticket {
    /// Its place at `now`, when the budget has one.
    pub fn try_take(&self, now: Instant) -> Option<Permit> {
        self.budget.grant(self.use_, now, true)
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        if self.use_ == Use::Download {
            let mut state = lock(&self.budget.state);
            state.downloads = state.downloads.saturating_sub(1);
        }
    }
}

/// Budgets by connector and account.
type Budgets = HashMap<(String, String), Arc<Budget>>;

/// The budgets of this process, by connector and account.
fn budgets() -> &'static Mutex<Budgets> {
    static BUDGETS: OnceLock<Mutex<Budgets>> = OnceLock::new();
    BUDGETS.get_or_init(Mutex::default)
}

/// The account a source belongs to: its id up to the first `/` (a
/// service whose source is the account itself, as Google Photos', is all
/// of it; an album of an account is `account/album`).
pub fn account_of(source: &SourceRef) -> &str {
    source.id.split('/').next().unwrap_or_default()
}

impl Budget {
    /// A budget of its own, outside the shared ones (tests).
    pub fn new(limits: Limits) -> Arc<Self> {
        Arc::new(Self {
            limits,
            state: Mutex::default(),
        })
    }

    /// The budget shared by every request to `source`'s account in this
    /// process, held to `limits` (its connector's). Limits changed since
    /// (a connector updated) start a new one.
    pub fn of(source: &SourceRef, limits: Limits) -> Arc<Self> {
        let key = (source.connector.clone(), account_of(source).to_owned());
        let mut all = lock(budgets());
        match all.get(&key) {
            Some(b) if b.limits == limits => b.clone(),
            _ => {
                let b = Self::new(limits);
                all.insert(key, b.clone());
                b
            }
        }
    }

    /// The limits it holds to.
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// Requests under way now.
    pub fn running(&self) -> u32 {
        lock(&self.state).running
    }

    /// A request of `use_` waits for its place: a download's ticket puts
    /// every thumbnail behind it until it is dropped.
    pub fn queue(self: &Arc<Self>, use_: Use) -> Ticket {
        if use_ == Use::Download {
            lock(&self.state).downloads += 1;
        }
        Ticket {
            budget: self.clone(),
            use_,
        }
    }

    /// A place for a request of `use_` at `now`, without waiting: `None`
    /// when the limits leave none, or a thumbnail while a download waits.
    pub fn try_take(self: &Arc<Self>, use_: Use, now: Instant) -> Option<Permit> {
        self.grant(use_, now, false)
    }

    /// A place for a request of `use_`, waited for: `None` once
    /// `cancelled` says so. `now` is the clock the limits are counted on.
    pub fn take(
        self: &Arc<Self>,
        use_: Use,
        now: &dyn Fn() -> Instant,
        cancelled: &dyn Fn() -> bool,
    ) -> Option<Permit> {
        let ticket = self.queue(use_);
        loop {
            if cancelled() {
                return None;
            }
            if let Some(permit) = ticket.try_take(now()) {
                return Some(permit);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// The service asked to wait (`retry_after`, else a minute): no
    /// request before.
    pub fn hold(&self, now: Instant, retry_after: Option<Duration>) {
        let until = now + retry_after.unwrap_or(MINUTE);
        let mut state = lock(&self.state);
        state.hold = Some(state.hold.map_or(until, |h| h.max(until)));
    }

    /// The daily quota ([`Limits::per_day`]) is spent at `now`: how long
    /// until its oldest request counted is a day old. `None`: requests are
    /// left for the day, or no daily quota is declared.
    pub fn day_spent(&self, now: Instant) -> Option<Duration> {
        let n = self.limits.per_day? as usize;
        let state = lock(&self.state);
        if day_count(&state, now) < n {
            return None;
        }
        let oldest = state
            .at
            .iter()
            .find(|t| now.saturating_duration_since(**t) < DAY && day_counts(&state, **t))?;
        Some(DAY.saturating_sub(now.saturating_duration_since(*oldest)))
    }

    /// The service's daily quota started again at `now` (its
    /// [`Limits::day_reset`] passed, or the person asked to go on): the
    /// requests before no longer count for the day.
    pub fn new_day(&self, now: Instant) {
        lock(&self.state).day_from = Some(now);
    }

    /// A place at `now`; `queued`: the request is one of the waiting
    /// counted (a download's ticket does not wait behind itself).
    fn grant(self: &Arc<Self>, use_: Use, now: Instant, queued: bool) -> Option<Permit> {
        let Limits {
            per_second,
            per_minute,
            per_day,
            parallel,
            ..
        } = self.limits;
        let mut state = lock(&self.state);
        if use_ == Use::Thumbnail && state.downloads > 0 {
            return None;
        }
        if use_ == Use::Download && !queued && state.downloads > 0 {
            return None;
        }
        if state.hold.is_some_and(|until| now < until) {
            return None;
        }
        if parallel.is_some_and(|n| state.running >= n) {
            return None;
        }
        let window = if per_day.is_some() {
            Some(DAY)
        } else if per_minute.is_some() {
            Some(MINUTE)
        } else if per_second.is_some() {
            Some(SECOND)
        } else {
            None
        };
        if let Some(window) = window {
            while state
                .at
                .front()
                .is_some_and(|t| now.saturating_duration_since(*t) >= window)
            {
                state.at.pop_front();
            }
            let within = |span: Duration| {
                state
                    .at
                    .iter()
                    .rev()
                    .take_while(|t| now.saturating_duration_since(**t) < span)
                    .count()
            };
            for (n, span) in [(per_second, SECOND), (per_minute, MINUTE)] {
                if n.is_some_and(|n| within(span) >= n as usize) {
                    return None;
                }
            }
            if per_day.is_some_and(|n| day_count(&state, now) >= n as usize) {
                return None;
            }
            state.at.push_back(now);
        }
        state.running += 1;
        Some(Permit(self.clone()))
    }
}

/// Whether a request made `at` counts for the day ([`State::day_from`]).
fn day_counts(state: &State, at: Instant) -> bool {
    state.day_from.is_none_or(|from| at >= from)
}

/// Requests counted for the day at `now`.
fn day_count(state: &State, now: Instant) -> usize {
    state
        .at
        .iter()
        .filter(|t| now.saturating_duration_since(**t) < DAY && day_counts(state, **t))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(per_second: Option<u32>, parallel: Option<u32>) -> Limits {
        Limits {
            per_second,
            parallel,
            ..Limits::default()
        }
    }

    #[test]
    fn parallel_places_are_given_back_when_dropped() {
        let b = Budget::new(limits(None, Some(2)));
        let t = Instant::now();
        let one = b.try_take(Use::Download, t).unwrap();
        let _two = b.try_take(Use::Thumbnail, t).unwrap();
        assert!(b.try_take(Use::Thumbnail, t).is_none());
        drop(one);
        assert!(b.try_take(Use::Thumbnail, t).is_some());
    }

    #[test]
    fn the_rate_a_second_and_a_minute_are_both_held() {
        let b = Budget::new(Limits {
            per_second: Some(2),
            per_minute: Some(3),
            ..Limits::default()
        });
        let t = Instant::now();
        assert!(b.try_take(Use::Thumbnail, t).is_some());
        assert!(b.try_take(Use::Download, t).is_some());
        assert!(b.try_take(Use::Download, t).is_none());
        let s = t + SECOND;
        assert!(b.try_take(Use::Download, s).is_some());
        assert!(b.try_take(Use::Download, s + SECOND).is_none());
        assert!(b.try_take(Use::Download, t + MINUTE).is_some());
    }

    #[test]
    fn a_waiting_download_goes_before_every_thumbnail() {
        let b = Budget::new(limits(Some(1), None));
        let t = Instant::now();
        let _first = b.try_take(Use::Thumbnail, t).unwrap();
        let ticket = b.queue(Use::Download);
        // The next second: the thumbnail waits behind the download.
        let s = t + SECOND;
        assert!(b.try_take(Use::Thumbnail, s).is_none());
        assert!(ticket.try_take(s).is_some());
        drop(ticket);
        let u = s + SECOND;
        assert!(b.try_take(Use::Thumbnail, u).is_some());
    }

    #[test]
    fn a_spent_day_says_so_until_a_new_day_starts() {
        let b = Budget::new(Limits {
            per_day: Some(2),
            ..Limits::default()
        });
        let t = Instant::now();
        assert!(b.try_take(Use::Download, t).is_some());
        assert_eq!(b.day_spent(t), None);
        let s = t + Duration::from_hours(1);
        assert!(b.try_take(Use::Download, s).is_some());
        assert!(b.try_take(Use::Download, s).is_none());
        assert_eq!(b.day_spent(s), Some(Duration::from_hours(23)));
        let u = s + Duration::from_hours(2);
        b.new_day(u);
        assert_eq!(b.day_spent(u), None);
        assert!(b.try_take(Use::Download, u).is_some());
    }

    #[test]
    fn a_wait_the_service_asked_for_holds_every_request() {
        let b = Budget::new(Limits::default());
        let t = Instant::now();
        b.hold(t, Some(Duration::from_secs(5)));
        assert!(
            b.try_take(Use::Download, t + Duration::from_secs(4))
                .is_none()
        );
        assert!(
            b.try_take(Use::Thumbnail, t + Duration::from_secs(5))
                .is_some()
        );
    }

    #[test]
    fn one_budget_per_account_shared_by_its_sources() {
        let source = |id: &str| SourceRef {
            connector: "budget-test".into(),
            id: id.into(),
            label: String::new(),
            root: None,
            capacity: None,
        };
        let l = limits(Some(4), Some(2));
        let a = Budget::of(&source("ann/album-1"), l);
        assert!(Arc::ptr_eq(&a, &Budget::of(&source("ann/album-2"), l)));
        assert!(Arc::ptr_eq(&a, &Budget::of(&source("ann"), l)));
        assert!(!Arc::ptr_eq(&a, &Budget::of(&source("bob"), l)));
    }
}

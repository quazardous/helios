//! Filling a buffer in row bands over the machine's threads.
//!
//! A per-pixel step on a full-resolution mosaic (a 25 MP raw: 100 MB of
//! `f32`) is bound by memory and by the page faults of its fresh output,
//! not by its arithmetic: one thread runs it at a fraction of the
//! machine's bandwidth. [`rows`] shares such a buffer's rows among the
//! machine's threads, a band at a time, and checks a cancel token before
//! each band, as the demosaic does. Each value is computed by the same
//! code whatever band holds it, so the result is the one-thread result,
//! bit for bit.
//!
//! The way darktable runs its per-pixel loops over the processor's cores
//! (`DT_OMP_FOR` row loops in `src/iop/*.c`, OpenMP's static schedule over
//! rows) is the model: <https://github.com/darktable-org/darktable>.

use std::sync::{Mutex, PoisonError};

use crate::cancel::{self, Token};

/// Values a band holds at least: below it, spawning a thread costs more
/// than the work it takes (a few microseconds against about a nanosecond
/// a value).
const MIN_BAND: usize = 1 << 16;

/// Bands per thread, so that a thread slowed down by another program does
/// not hold the rest back.
const BANDS_PER_THREAD: usize = 4;

/// The threads a band loop may use: the machine's.
pub fn threads() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
}

/// Calls `work(first, band)` on bands of whole rows of `data`, `row_len`
/// values a row, `first` the index of the band's first row: over the
/// machine's threads when there is enough work, on the calling thread
/// otherwise. `cancel` is checked before each band; once it is set, no
/// band starts any more and the call returns [`cancel::CANCELLED`].
pub fn rows<T: Send>(
    data: &mut [T],
    row_len: usize,
    cancel: &Token,
    work: impl Fn(usize, &mut [T]) + Sync,
) -> Result<(), String> {
    rows_over(data, row_len, threads(), cancel, work)
}

/// [`rows`] over at most `threads` threads.
pub fn rows_over<T: Send>(
    data: &mut [T],
    row_len: usize,
    threads: usize,
    cancel: &Token,
    work: impl Fn(usize, &mut [T]) + Sync,
) -> Result<(), String> {
    let stopped = || Err(cancel::CANCELLED.to_string());
    if row_len == 0 || data.is_empty() {
        return if cancel.is_cancelled() {
            stopped()
        } else {
            Ok(())
        };
    }
    let count = data.len() / row_len;
    let threads = threads.max(1);
    let min_rows = MIN_BAND.div_ceil(row_len);
    let per_band = count
        .div_ceil(threads * BANDS_PER_THREAD)
        .max(min_rows)
        .max(1);
    let bands = count.div_ceil(per_band);
    if threads == 1 || bands <= 1 {
        for (i, band) in data.chunks_mut(per_band * row_len).enumerate() {
            if cancel.is_cancelled() {
                return stopped();
            }
            work(i * per_band, band);
        }
        return Ok(());
    }
    let next = Mutex::new(data.chunks_mut(per_band * row_len).enumerate());
    let worker = || -> Result<(), String> {
        loop {
            let taken = next.lock().unwrap_or_else(PoisonError::into_inner).next();
            let Some((i, band)) = taken else {
                return Ok(());
            };
            if cancel.is_cancelled() {
                return stopped();
            }
            work(i * per_band, band);
        }
    };
    std::thread::scope(|s| {
        let helpers: Vec<_> = (1..threads.min(bands)).map(|_| s.spawn(worker)).collect();
        // The calling thread takes bands too.
        let own = worker();
        helpers
            .into_iter()
            .map(|h| h.join().unwrap_or_else(|p| std::panic::resume_unwind(p)))
            .chain(std::iter::once(own))
            .collect::<Result<Vec<()>, String>>()
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_row_is_worked_once_whatever_the_threads() {
        let (row_len, count) = (1000, 700);
        let one = {
            let mut data = vec![0u32; row_len * count];
            rows_over(&mut data, row_len, 1, &Token::new(), |first, band| {
                for (i, row) in band.chunks_mut(row_len).enumerate() {
                    row.fill((first + i) as u32 + 1);
                }
            })
            .unwrap();
            data
        };
        for threads in [2, 3, 16] {
            let mut data = vec![0u32; row_len * count];
            rows_over(&mut data, row_len, threads, &Token::new(), |first, band| {
                for (i, row) in band.chunks_mut(row_len).enumerate() {
                    row.fill((first + i) as u32 + 1);
                }
            })
            .unwrap();
            assert_eq!(data, one, "{threads} threads");
        }
        assert_eq!(one[row_len * 699], 700);
    }

    #[test]
    fn a_cancelled_token_starts_no_band() {
        let token = Token::new();
        token.cancel();
        let mut data = vec![0u8; 1 << 20];
        let err = rows_over(&mut data, 1024, 4, &token, |_, band| band.fill(1)).unwrap_err();
        assert_eq!(err, cancel::CANCELLED);
        assert!(data.iter().all(|v| *v == 0));
    }

    #[test]
    fn empty_buffers_are_fine() {
        let mut data: Vec<f32> = Vec::new();
        rows(&mut data, 4, &Token::new(), |_, _| unreachable!()).unwrap();
        rows(&mut data, 0, &Token::new(), |_, _| unreachable!()).unwrap();
    }
}

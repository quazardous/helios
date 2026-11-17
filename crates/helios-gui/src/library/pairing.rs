//! RAW+JPEG pairs in the Library (`docs/design/raw-jpeg-pairs.md`):
//! "Pair RAW+JPEG Files" makes each pair found among the selection (or
//! two photos as they are, or the whole Library from the health check's
//! offer) one photo, the JPEG its camera file; "Unpair Files" makes the
//! files photos again. Each is asked of the host, which sends it to the
//! node (`library.pairs.pair`, `library.pairs.unpair`) and records the
//! step it answers in the library's history: Ctrl+Z takes it back.

use super::Library;
use crate::library_write::WriteAsk;

/// Pairs RAW+JPEG files: the pairs among photos (`images`, else the
/// selection; `all`: the whole Library), or two photos as they are
/// (`exact`).
pub const PAIR_COMMAND: &str = helios_library::health::PAIR_FILES;

/// Unpairs photos (`images`, else the selection): each file a photo of
/// its own again.
pub const UNPAIR_COMMAND: &str = "library.files.unpair";

/// What the commands asked of pairs, for the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairAsk {
    /// Pair the pairs among `images` (none: the whole Library), or the
    /// two as they are when `exact`.
    Pair {
        images: Option<Vec<String>>,
        exact: bool,
    },
    /// Unpair `images`.
    Unpair { images: Vec<String> },
}

impl Library {
    /// The photos named the library holds, else the selection.
    fn named_or_selected(&self, images: Option<Vec<String>>) -> Result<Vec<String>, String> {
        match images {
            Some(ids) => {
                let keys: Vec<String> = ids.into_iter().filter(|i| self.holds(i)).collect();
                if keys.is_empty() {
                    Err("none of the `images` is in the library".into())
                } else {
                    Ok(keys)
                }
            }
            None => self.targets(),
        }
    }

    /// Asks the RAW+JPEG pairs among `images` (else the selection; with
    /// `all`, the whole Library) paired, or the two photos as they are
    /// with `exact`; how many photos are looked at (none for the whole
    /// Library). Which are pairs is the node's to say: it knows their
    /// files.
    ///
    /// # Errors
    /// Fewer than two photos, or `exact` with other than two.
    pub fn ask_pair(
        &mut self,
        images: Option<Vec<String>>,
        all: bool,
        exact: bool,
    ) -> Result<Option<usize>, String> {
        let images = if all {
            None
        } else {
            let keys = self.named_or_selected(images)?;
            if keys.len() < 2 {
                return Err("a pair needs two photos".into());
            }
            if exact && keys.len() != 2 {
                return Err("pairing photos as they are takes two photos".into());
            }
            Some(keys)
        };
        let count = images.as_ref().map(Vec::len);
        self.ask_write(WriteAsk::Pair(
            PairAsk::Pair { images, exact },
            crate::library_history::current_batch(),
        ));
        Ok(count)
    }

    /// Asks `images` (else the selection) unpaired; how many photos.
    ///
    /// # Errors
    /// No photo.
    pub fn ask_unpair(&mut self, images: Option<Vec<String>>) -> Result<usize, String> {
        let keys = self.named_or_selected(images)?;
        let count = keys.len();
        self.ask_write(WriteAsk::Pair(
            PairAsk::Unpair { images: keys },
            crate::library_history::current_batch(),
        ));
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_and_unpairing_are_asked_of_the_host() {
        let mut lib = Library::default();
        lib.set_entries(
            ["a", "b", "c"]
                .into_iter()
                .map(|id| crate::library::Entry {
                    id: id.into(),
                    name: format!("{id}.jpg"),
                    ..crate::library::Entry::default()
                })
                .collect(),
        );
        assert!(lib.ask_pair(None, false, false).is_err(), "no selection");
        lib.select_ids(&["a".into()]);
        assert!(lib.ask_pair(None, false, false).is_err(), "one photo");
        lib.select_ids(&["a".into(), "b".into(), "c".into()]);
        assert!(lib.ask_pair(None, false, true).is_err(), "exact takes two");
        assert_eq!(lib.ask_pair(None, false, false), Ok(Some(3)));
        assert_eq!(lib.ask_pair(None, true, false), Ok(None), "the Library");
        assert_eq!(lib.ask_unpair(Some(vec!["b".into(), "z".into()])), Ok(1));
        assert!(lib.ask_unpair(Some(vec!["z".into()])).is_err());
        let asks: Vec<PairAsk> = lib.asked_pair_asks().into_iter().map(|(a, _)| a).collect();
        assert_eq!(
            asks,
            vec![
                PairAsk::Pair {
                    images: Some(vec!["a".into(), "b".into(), "c".into()]),
                    exact: false,
                },
                PairAsk::Pair {
                    images: None,
                    exact: false,
                },
                PairAsk::Unpair {
                    images: vec!["b".into()],
                },
            ]
        );
        assert!(lib.asked_pair_asks().is_empty());
    }
}

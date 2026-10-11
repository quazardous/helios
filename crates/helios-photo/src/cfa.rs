//! Color filter arrays: which color each photosite of a sensor records.

use serde::{Deserialize, Serialize};

/// The color a photosite records.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Channel {
    Red,
    Green,
    Blue,
}

impl Channel {
    fn letter(self) -> char {
        match self {
            Channel::Red => 'R',
            Channel::Green => 'G',
            Channel::Blue => 'B',
        }
    }

    fn from_letter(c: char) -> Option<Self> {
        match c {
            'R' => Some(Channel::Red),
            'G' => Some(Channel::Green),
            'B' => Some(Channel::Blue),
            _ => None,
        }
    }
}

/// A repeating pattern of filters, anchored at the image origin (0, 0).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Cfa {
    /// 2 x 2, row by row: `RGGB` means red at (0, 0), green at (1, 0) and
    /// (0, 1), blue at (1, 1).
    Bayer([Channel; 4]),
    /// Fujifilm's 6 x 6 pattern, row by row.
    XTrans([[Channel; 6]; 6]),
}

impl Cfa {
    /// Parses `RGGB`-style Bayer names, or 36 letters (six rows of six,
    /// optionally separated by `/`) for X-Trans.
    pub fn parse(name: &str) -> Result<Self, String> {
        let letters: Vec<Channel> = name
            .chars()
            .filter(|c| *c != '/')
            .map(|c| {
                Channel::from_letter(c).ok_or_else(|| format!("`{c}` is not R, G or B in `{name}`"))
            })
            .collect::<Result<_, _>>()?;
        match letters.len() {
            4 => Ok(Cfa::Bayer([letters[0], letters[1], letters[2], letters[3]])),
            36 => {
                let mut rows = [[Channel::Green; 6]; 6];
                for (i, c) in letters.into_iter().enumerate() {
                    rows[i / 6][i % 6] = c;
                }
                Ok(Cfa::XTrans(rows))
            }
            n => Err(format!(
                "`{name}`: a pattern has 4 (Bayer) or 36 (X-Trans) filters, not {n}"
            )),
        }
    }

    /// The name [`Cfa::parse`] reads back.
    pub fn name(&self) -> String {
        match self {
            Cfa::Bayer(p) => p.iter().map(|c| c.letter()).collect(),
            Cfa::XTrans(rows) => rows
                .iter()
                .map(|r| r.iter().map(|c| c.letter()).collect::<String>())
                .collect::<Vec<_>>()
                .join("/"),
        }
    }

    /// Side of the repeating pattern.
    pub fn period(&self) -> i64 {
        match self {
            Cfa::Bayer(_) => 2,
            Cfa::XTrans(_) => 6,
        }
    }

    /// The color of the photosite at absolute image coordinates.
    pub fn color_at(&self, x: i64, y: i64) -> Channel {
        let (px, py) = (
            x.rem_euclid(self.period()) as usize,
            y.rem_euclid(self.period()) as usize,
        );
        match self {
            Cfa::Bayer(p) => p[py * 2 + px],
            Cfa::XTrans(rows) => rows[py][px],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bayer_colors_repeat_from_the_origin() {
        let cfa = Cfa::parse("RGGB").unwrap();
        assert_eq!(cfa.color_at(0, 0), Channel::Red);
        assert_eq!(cfa.color_at(1, 0), Channel::Green);
        assert_eq!(cfa.color_at(0, 1), Channel::Green);
        assert_eq!(cfa.color_at(1, 1), Channel::Blue);
        assert_eq!(cfa.color_at(2, 2), Channel::Red);
        assert_eq!(
            cfa.color_at(-1, -1),
            Channel::Blue,
            "negative coordinates wrap too"
        );
        assert_eq!(cfa.name(), "RGGB");
    }

    #[test]
    fn xtrans_round_trips_through_its_name() {
        let name = "GGRGGB/GGBGGR/BRGRBG/GGBGGR/GGRGGB/RBGBRG";
        let cfa = Cfa::parse(name).unwrap();
        assert_eq!(cfa.name(), name);
        assert_eq!(cfa.period(), 6);
        assert_eq!(cfa.color_at(2, 0), Channel::Red);
        assert_eq!(cfa.color_at(8, 6), Channel::Red, "repeats every 6");
    }

    #[test]
    fn bad_patterns_say_why() {
        assert!(
            Cfa::parse("RGXB")
                .unwrap_err()
                .contains("`X` is not R, G or B")
        );
        assert!(Cfa::parse("RGB").unwrap_err().contains("not 3"));
    }
}

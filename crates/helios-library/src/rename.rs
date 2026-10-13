//! File names by a pattern, for an import that copies: `{date}-{seq}`
//! makes `20240601-0001.cr2`.
//!
//! Fields: `{date}` (`YYYYMMDD`), `{time}` (`HHMMSS`), `{year}`, `{month}`,
//! `{day}`, `{camera}` (the model), `{maker}`, `{frame}` (the camera's
//! frame number), `{stem}` (the original name without its extension),
//! `{seq}` (the file's place in the import, from 1, four digits) and
//! `{seq:N}` (N digits). The extension is the original one, always kept.
//! A field with no value (no capture date) writes `unknown`. Names never
//! hold a path separator.

use crate::signals::Signals;

/// A checked pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pattern {
    parts: Vec<Part>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Part {
    Text(String),
    Date,
    Time,
    Year,
    Month,
    Day,
    Camera,
    Maker,
    Frame,
    Stem,
    Seq(usize),
}

/// What a name is made from.
pub struct Naming<'a> {
    pub stem: &'a str,
    pub signals: &'a Signals,
    /// The file's place in the import, from 1.
    pub seq: u64,
}

impl Pattern {
    pub fn parse(text: &str) -> Result<Pattern, String> {
        let mut parts = Vec::new();
        let mut rest = text;
        while let Some(open) = rest.find('{') {
            if open > 0 {
                parts.push(Part::Text(rest[..open].to_string()));
            }
            let close = rest[open..]
                .find('}')
                .ok_or_else(|| format!("`{text}`: a `{{` is not closed"))?;
            let field = &rest[open + 1..open + close];
            parts.push(match field {
                "date" => Part::Date,
                "time" => Part::Time,
                "year" => Part::Year,
                "month" => Part::Month,
                "day" => Part::Day,
                "camera" => Part::Camera,
                "maker" => Part::Maker,
                "frame" => Part::Frame,
                "stem" => Part::Stem,
                "seq" => Part::Seq(4),
                other => match other.strip_prefix("seq:").map(str::parse::<usize>) {
                    Some(Ok(n)) if (1..=9).contains(&n) => Part::Seq(n),
                    _ => {
                        return Err(format!(
                            "`{text}`: unknown field `{{{other}}}` (date, time, year, month, day, camera, maker, frame, stem, seq, seq:N)"
                        ));
                    }
                },
            });
            rest = &rest[open + close + 1..];
        }
        if !rest.is_empty() {
            parts.push(Part::Text(rest.to_string()));
        }
        if parts.is_empty() {
            return Err("an empty pattern names nothing".into());
        }
        if text.contains(['/', '\\']) {
            return Err(format!("`{text}`: a name holds no path separator"));
        }
        Ok(Pattern { parts })
    }

    /// The name for one file, without its extension.
    pub fn name(&self, n: &Naming<'_>) -> String {
        let date = n.signals.date();
        let time = n.signals.time_of_day();
        let unknown = || "unknown".to_string();
        let mut out = String::new();
        for part in &self.parts {
            let piece = match part {
                Part::Text(t) => t.clone(),
                Part::Date => date.map_or_else(unknown, |(y, m, d)| format!("{y:04}{m:02}{d:02}")),
                Part::Time => time.map_or_else(unknown, |(h, m, s)| format!("{h:02}{m:02}{s:02}")),
                Part::Year => date.map_or_else(unknown, |(y, _, _)| format!("{y:04}")),
                Part::Month => date.map_or_else(unknown, |(_, m, _)| format!("{m:02}")),
                Part::Day => date.map_or_else(unknown, |(_, _, d)| format!("{d:02}")),
                Part::Camera => n.signals.model.clone().unwrap_or_else(unknown),
                Part::Maker => n.signals.maker.clone().unwrap_or_else(unknown),
                Part::Frame => n.signals.frame.map_or_else(unknown, |f| f.to_string()),
                Part::Stem => n.stem.to_string(),
                Part::Seq(width) => format!("{:0w$}", n.seq, w = *width),
            };
            out.push_str(&clean(&piece));
        }
        out
    }
}

/// A piece of a file name: separators and control characters become `-`,
/// spaces `-` too, so names stay one word.
fn clean(piece: &str) -> String {
    piece
        .trim()
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | ' ') {
                '-'
            } else {
                c
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pattern_names_by_date_camera_and_sequence() {
        let signals = Signals {
            taken: crate::signals::exif_time("2024:06:01 08:09:10", None),
            frame: Some(42),
            maker: Some("Canon".into()),
            model: Some("EOS R5".into()),
            ..Signals::default()
        };
        let naming = Naming {
            stem: "IMG_0042",
            signals: &signals,
            seq: 7,
        };
        let p = Pattern::parse("{date}-{time}_{camera}_{seq}").unwrap();
        assert_eq!(p.name(&naming), "20240601-080910_EOS-R5_0007");
        let p = Pattern::parse("{year}/{stem}");
        assert!(p.is_err());
        let p = Pattern::parse("{stem}-{frame}-{seq:2}").unwrap();
        assert_eq!(p.name(&naming), "IMG_0042-42-07");
        assert!(Pattern::parse("{nope}").is_err());
        assert!(Pattern::parse("{date").is_err());
        let none = Signals::default();
        let p = Pattern::parse("{date}").unwrap();
        let naming = Naming {
            signals: &none,
            ..naming
        };
        assert_eq!(p.name(&naming), "unknown");
    }
}

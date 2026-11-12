//! An item read as a `Read + Seek` through its connector's fetch from an
//! offset: what a reader that seeks (a raw's previews read without its
//! sensor data) needs from a source it cannot open, when the connector can
//! resume (`caps.resume`) and the item's size is known.

use std::io::{self, Read, Seek, SeekFrom};

use crate::{Cancel, Connector, FetchSink, ItemRef};

/// An item's bytes as a `Read + Seek`: each read is one fetch from the
/// position, stopped once the buffer is full.
pub struct Ranged<'a> {
    connector: &'a dyn Connector,
    item: &'a ItemRef,
    cancel: &'a Cancel,
    len: u64,
    pos: u64,
}

impl<'a> Ranged<'a> {
    /// `len` is the item's size (`Item::size`, known before a fetch when
    /// `caps.sizes`).
    pub fn new(
        connector: &'a dyn Connector,
        item: &'a ItemRef,
        len: u64,
        cancel: &'a Cancel,
    ) -> Self {
        Self {
            connector,
            item,
            cancel,
            len,
            pos: 0,
        }
    }
}

/// Takes bytes into a buffer, then refuses more: the fetch stops.
struct Fill<'b> {
    buf: &'b mut [u8],
    got: usize,
}

impl FetchSink for Fill<'_> {
    fn accept(&mut self, chunk: &[u8]) -> io::Result<()> {
        let n = chunk.len().min(self.buf.len() - self.got);
        self.buf[self.got..self.got + n].copy_from_slice(&chunk[..n]);
        self.got += n;
        if self.got == self.buf.len() {
            return Err(io::Error::other("enough"));
        }
        Ok(())
    }
}

impl Read for Ranged<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.len.saturating_sub(self.pos);
        let want = buf.len().min(usize::try_from(left).unwrap_or(usize::MAX));
        if want == 0 {
            return Ok(0);
        }
        let mut fill = Fill {
            buf: &mut buf[..want],
            got: 0,
        };
        let fetched = self
            .connector
            .fetch(self.item, self.pos, &mut fill, self.cancel);
        // The sink's refusal ends a fetch that had what was asked.
        if fill.got < want
            && let Err(e) = fetched
        {
            return Err(io::Error::other(e.to_string()));
        }
        self.pos += fill.got as u64;
        Ok(fill.got)
    }
}

impl Seek for Ranged<'_> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let to = match pos {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => self.len.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        self.pos =
            to.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek before 0"))?;
        Ok(self.pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::folder::FolderProvider;
    use crate::local::source_at;
    use crate::{ConnectorProvider, ConnectorServices, ItemId, Settings};

    #[test]
    fn an_item_reads_and_seeks_through_fetches_from_an_offset() {
        let dir = std::env::temp_dir().join(format!("helios-ranged-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bytes: Vec<u8> = (0..300_000u32).map(|i| (i % 253) as u8).collect();
        std::fs::write(dir.join("a.nef"), &bytes).unwrap();
        let connector = FolderProvider
            .create(&Settings::Null, &ConnectorServices::default())
            .unwrap();
        let item = ItemRef {
            source: source_at("folder", &dir, "test".into(), None),
            id: ItemId("a.nef".into()),
        };
        let cancel = Cancel::new();
        let mut reader = Ranged::new(connector.as_ref(), &item, bytes.len() as u64, &cancel);
        let mut part = vec![0u8; 1000];
        reader.seek(SeekFrom::Start(200_000)).unwrap();
        reader.read_exact(&mut part).unwrap();
        assert_eq!(part, bytes[200_000..201_000]);
        assert_eq!(reader.seek(SeekFrom::End(-10)).unwrap(), 299_990);
        let mut tail = Vec::new();
        reader.read_to_end(&mut tail).unwrap();
        assert_eq!(tail, bytes[299_990..]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

//! The metadata region format every family shares: a count, then
//! `(kind, length, payload)` sections in order, read without panics.

use alloc::vec::Vec;

use crate::Code;

/// A reader over a byte slice that never panics.
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    /// A reader at the start of `bytes`.
    pub(crate) const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    /// The bytes not read yet.
    pub(crate) const fn rest(&self) -> &'a [u8] {
        self.bytes
    }

    /// The next `len` bytes, `None` when fewer remain.
    pub(crate) fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let head = self.bytes.get(..len)?;
        self.bytes = self.bytes.get(len..)?;
        Some(head)
    }

    /// The next little-endian `u16`.
    pub(crate) fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }

    /// The next little-endian `u32`.
    pub(crate) fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
}

/// The payloads of a region that holds exactly `kinds` in order. `None` for
/// any other region.
pub(crate) fn sections<const K: usize>(region: &[u8], kinds: [u16; K]) -> Option<[&[u8]; K]> {
    let mut reader = Reader::new(region);
    if usize::from(reader.u16()?) != K {
        return None;
    }
    let mut payloads = [&[][..]; K];
    for (slot, kind) in payloads.iter_mut().zip(kinds) {
        if reader.u16()? != kind {
            return None;
        }
        let len = usize::try_from(reader.u32()?).ok()?;
        *slot = reader.take(len)?;
    }
    reader.rest().is_empty().then_some(())?;
    Some(payloads)
}

/// A metadata region from `(kind, payload)` sections, in order.
pub(crate) fn region(sections: &[(u16, Vec<u8>)]) -> Result<Vec<u8>, Code> {
    let overflow = Code::BoundsExceeded;
    let count = u16::try_from(sections.len()).map_err(|_| overflow)?;
    let mut region = count.to_le_bytes().to_vec();
    for (kind, payload) in sections {
        region.extend_from_slice(&kind.to_le_bytes());
        region.extend_from_slice(
            &u32::try_from(payload.len())
                .map_err(|_| overflow)?
                .to_le_bytes(),
        );
        region.extend_from_slice(payload);
    }
    Ok(region)
}

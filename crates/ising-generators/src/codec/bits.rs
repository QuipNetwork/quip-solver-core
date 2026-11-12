use super::{filled, CodecError};
use alloc::vec::Vec;

pub(crate) fn byte_len(bits: usize) -> Result<usize, CodecError> {
    Ok(bits.checked_add(7).ok_or(CodecError::BoundsExceeded)? / 8)
}
pub(crate) fn check_bit_array(bytes: &[u8], bits: usize) -> Result<(), CodecError> {
    if bytes.len() != byte_len(bits)? {
        return Err(CodecError::Length);
    }
    if !bits.is_multiple_of(8) && bytes.last().is_some_and(|last| last >> (bits % 8) != 0) {
        return Err(CodecError::Padding);
    }
    Ok(())
}
pub(crate) struct BitWriter {
    bytes: Vec<u8>,
    bits: usize,
    at: usize,
}
impl BitWriter {
    pub(crate) fn new(bits: usize) -> Result<Self, CodecError> {
        Ok(Self {
            bytes: filled(byte_len(bits)?, 0)?,
            bits,
            at: 0,
        })
    }
    pub(crate) fn push(&mut self, value: u8, width: u8) -> Result<(), CodecError> {
        if width > 4 || u16::from(value) >= (1u16 << width) {
            return Err(CodecError::InvalidSymbol);
        }
        let end = self
            .at
            .checked_add(usize::from(width))
            .ok_or(CodecError::BoundsExceeded)?;
        if end > self.bits {
            return Err(CodecError::Length);
        }
        for shift in 0..width {
            let byte = self.bytes.get_mut(self.at / 8).ok_or(CodecError::Length)?;
            *byte |= ((value >> shift) & 1) << (self.at % 8);
            self.at += 1;
        }
        Ok(())
    }
    pub(crate) fn finish(self) -> Result<Vec<u8>, CodecError> {
        if self.at != self.bits {
            return Err(CodecError::Length);
        }
        Ok(self.bytes)
    }
}
pub(crate) struct BitReader<'a> {
    bytes: &'a [u8],
    bits: usize,
    at: usize,
}
impl<'a> BitReader<'a> {
    pub(crate) fn new(bytes: &'a [u8], bits: usize) -> Result<Self, CodecError> {
        check_bit_array(bytes, bits)?;
        Ok(Self { bytes, bits, at: 0 })
    }
    pub(crate) fn read(&mut self, width: u8) -> Result<u8, CodecError> {
        if width > 4 {
            return Err(CodecError::InvalidSymbol);
        }
        let end = self
            .at
            .checked_add(usize::from(width))
            .ok_or(CodecError::BoundsExceeded)?;
        if end > self.bits {
            return Err(CodecError::Length);
        }
        let mut value = 0;
        for shift in 0..width {
            value |= ((self.bytes.get(self.at / 8).ok_or(CodecError::Length)? >> (self.at % 8))
                & 1)
                << shift;
            self.at += 1;
        }
        Ok(value)
    }
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test assertions must fail independently of setup errors"
)]
mod tests {
    use super::*;
    #[test]
    fn bit_layout_and_bounds() -> Result<(), CodecError> {
        let mut writer = BitWriter::new(3)?;
        for bit in [1, 0, 1] {
            writer.push(bit, 1)?;
        }
        assert_eq!(writer.finish()?, [5]);
        let mut reader = BitReader::new(&[5], 3)?;
        for bit in [1, 0, 1] {
            assert_eq!(reader.read(1)?, bit);
        }
        assert!(matches!(
            BitReader::new(&[133], 3),
            Err(CodecError::Padding)
        ));
        assert!(matches!(BitReader::new(&[], 3), Err(CodecError::Length)));
        assert!(matches!(
            BitReader::new(&[5, 0], 3),
            Err(CodecError::Length)
        ));
        assert!(matches!(
            BitWriter::new(usize::MAX),
            Err(CodecError::BoundsExceeded)
        ));
        assert!(matches!(
            BitReader::new(&[], usize::MAX),
            Err(CodecError::BoundsExceeded)
        ));
        assert_eq!(BitWriter::new(0)?.finish()?, []);
        let mut byte = BitWriter::new(8)?;
        byte.push(15, 4)?;
        byte.push(15, 4)?;
        assert_eq!(byte.finish()?, [255]);
        Ok(())
    }
}

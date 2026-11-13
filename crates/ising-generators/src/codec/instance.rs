//! Borrowed prefix preflight and lossless integer columns.
pub(crate) use super::bits::check_bit_array;
use super::{bits::byte_len, filled, CodecError};
use crate::instance::{Instance, InstanceLimits, InstanceParts};
use alloc::vec::Vec;

pub(crate) struct Column<'a> {
    data: &'a [u8],
    pub(crate) count: usize,
    width: u8,
}
impl Column<'_> {
    pub(crate) fn get(&self, index: usize) -> Result<u32, CodecError> {
        if index >= self.count {
            return Err(CodecError::Length);
        }
        let start = index
            .checked_mul(usize::from(self.width))
            .ok_or(CodecError::BoundsExceeded)?;
        let mut value = 0;
        for bit in 0..usize::from(self.width) {
            let at = start + bit;
            value |= u32::from((self.data.get(at / 8).ok_or(CodecError::Length)? >> (at % 8)) & 1)
                << bit;
        }
        Ok(value)
    }
    pub(crate) fn minimal(&self) -> Result<(), CodecError> {
        let mut max = 0;
        for i in 0..self.count {
            max |= self.get(i)?;
        }
        if u32::from(self.width) != 32 - max.leading_zeros() {
            return Err(CodecError::NonCanonical);
        }
        Ok(())
    }
}
pub(crate) fn read_column(data: &[u8], count: usize) -> Result<(Column<'_>, usize), CodecError> {
    let width = *data.first().ok_or(CodecError::Length)?;
    if width > 32 {
        return Err(CodecError::InvalidSymbol);
    }
    let bits = count
        .checked_mul(usize::from(width))
        .ok_or(CodecError::BoundsExceeded)?;
    let end = byte_len(bits)?
        .checked_add(1)
        .ok_or(CodecError::BoundsExceeded)?;
    let payload = data.get(1..end).ok_or(CodecError::Length)?;
    check_bit_array(payload, bits)?;
    Ok((
        Column {
            data: payload,
            count,
            width,
        },
        end,
    ))
}
pub(crate) fn write_column(values: &[u32], out: &mut Vec<u8>) -> Result<(), CodecError> {
    let max = values.iter().copied().max().unwrap_or(0);
    let width = u8::try_from(32 - max.leading_zeros()).map_err(|_| CodecError::BoundsExceeded)?;
    let bits = values
        .len()
        .checked_mul(usize::from(width))
        .ok_or(CodecError::BoundsExceeded)?;
    let len = byte_len(bits)?
        .checked_add(1)
        .ok_or(CodecError::BoundsExceeded)?;
    out.try_reserve(len).map_err(|_| CodecError::Allocation)?;
    out.push(width);
    let start = out.len();
    out.resize(
        start
            .checked_add(len - 1)
            .ok_or(CodecError::BoundsExceeded)?,
        0,
    );
    let mut at = 0;
    for &value in values {
        for bit in 0..width {
            *out.get_mut(start + at / 8).ok_or(CodecError::Length)? |=
                u8::from(value & (1 << bit) != 0) << (at % 8);
            at += 1;
        }
    }
    Ok(())
}
fn take<const N: usize>(bytes: &[u8], at: usize) -> Result<[u8; N], CodecError> {
    bytes
        .get(at..at.checked_add(N).ok_or(CodecError::BoundsExceeded)?)
        .ok_or(CodecError::Length)?
        .try_into()
        .map_err(|_| CodecError::Length)
}
fn append(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), CodecError> {
    out.try_reserve(bytes.len())
        .map_err(|_| CodecError::Allocation)?;
    out.extend_from_slice(bytes);
    Ok(())
}
struct Palette<'a> {
    values: &'a [u8],
    ranks: Column<'a>,
    count: usize,
}
impl Palette<'_> {
    fn value(&self, index: usize) -> Result<i32, CodecError> {
        if index >= self.count {
            return Err(CodecError::InvalidSymbol);
        }
        Ok(i32::from_le_bytes(take(
            self.values,
            index.checked_mul(4).ok_or(CodecError::BoundsExceeded)?,
        )?))
    }
    fn minimal(&self) -> Result<(), CodecError> {
        self.ranks.minimal()?;
        let mut used = filled(self.count, false)?;
        let mut previous = None;
        for i in 0..self.count {
            let value = self.value(i)?;
            if previous.is_some_and(|p| p >= value) {
                return Err(CodecError::NonCanonical);
            }
            previous = Some(value);
        }
        for i in 0..self.ranks.count {
            *used
                .get_mut(self.ranks.get(i)? as usize)
                .ok_or(CodecError::InvalidSymbol)? = true;
        }
        if used.contains(&false) {
            return Err(CodecError::NonCanonical);
        }
        Ok(())
    }
    fn decode(&self) -> Result<Vec<i32>, CodecError> {
        let mut values = filled(self.ranks.count, 0)?;
        for (i, value) in values.iter_mut().enumerate() {
            *value = self.value(self.ranks.get(i)? as usize)?;
        }
        Ok(values)
    }
}
fn palette<'a>(
    data: &'a [u8],
    count: usize,
    limits: &InstanceLimits,
) -> Result<(Palette<'a>, usize), CodecError> {
    let k = u32::from_le_bytes(take(data, 0)?) as usize;
    if k > count {
        return Err(CodecError::BoundsExceeded);
    }
    if count != 0 && k == 0 {
        return Err(CodecError::InvalidSymbol);
    }
    let end = k
        .checked_mul(4)
        .and_then(|n| n.checked_add(4))
        .ok_or(CodecError::BoundsExceeded)?;
    let values = data.get(4..end).ok_or(CodecError::Length)?;
    for bytes in values.chunks_exact(4) {
        if i32::from_le_bytes(bytes.try_into().map_err(|_| CodecError::Length)?).unsigned_abs()
            > limits.max_coefficient()
        {
            return Err(CodecError::BoundsExceeded);
        }
    }
    let (ranks, len) = read_column(data.get(end..).ok_or(CodecError::Length)?, count)?;
    for i in 0..count {
        if ranks.get(i)? as usize >= k {
            return Err(CodecError::InvalidSymbol);
        }
    }
    Ok((
        Palette {
            values,
            ranks,
            count: k,
        },
        end.checked_add(len).ok_or(CodecError::BoundsExceeded)?,
    ))
}

// Four stable byte passes order signed coefficients and retain their original positions.
fn write_palette(values: &[i32], out: &mut Vec<u8>) -> Result<(), CodecError> {
    let mut entries = Vec::new();
    entries
        .try_reserve_exact(values.len())
        .map_err(|_| CodecError::Allocation)?;
    entries.extend(values.iter().copied().enumerate().map(|(i, v)| (v, i)));
    let mut scratch = filled(values.len(), (0, 0))?;
    for shift in [0, 8, 16, 24] {
        let mut offsets = [0usize; 256];
        for &(v, _) in &entries {
            let key = u32::from_ne_bytes(v.to_ne_bytes()) ^ 0x8000_0000;
            *offsets
                .get_mut(((key >> shift) & 255) as usize)
                .ok_or(CodecError::InvalidInstance)? += 1;
        }
        let mut sum = 0;
        for slot in &mut offsets {
            let count = *slot;
            *slot = sum;
            sum += count;
        }
        for &(v, i) in &entries {
            let key = u32::from_ne_bytes(v.to_ne_bytes()) ^ 0x8000_0000;
            let at = offsets
                .get_mut(((key >> shift) & 255) as usize)
                .ok_or(CodecError::InvalidInstance)?;
            *scratch.get_mut(*at).ok_or(CodecError::InvalidInstance)? = (v, i);
            *at += 1;
        }
        core::mem::swap(&mut entries, &mut scratch);
    }
    let mut palette = Vec::new();
    palette
        .try_reserve_exact(values.len())
        .map_err(|_| CodecError::Allocation)?;
    let mut ranks = filled(values.len(), 0)?;
    for (v, i) in entries {
        if palette.last() != Some(&v) {
            palette.push(v);
        }
        *ranks.get_mut(i).ok_or(CodecError::InvalidInstance)? =
            u32::try_from(palette.len() - 1).map_err(|_| CodecError::BoundsExceeded)?;
    }
    append(
        out,
        &u32::try_from(palette.len())
            .map_err(|_| CodecError::BoundsExceeded)?
            .to_le_bytes(),
    )?;
    for v in palette {
        append(out, &v.to_le_bytes())?;
    }
    write_column(&ranks, out)
}

pub(crate) struct InstancePrefix<'a> {
    head: &'a [u8],
    n: usize,
    e: usize,
    ids: Column<'a>,
    endpoints: Column<'a>,
    h: Palette<'a>,
    j: Palette<'a>,
    metadata_start: usize,
    descriptor: &'a [u8; 32],
}
impl InstancePrefix<'_> {
    pub(crate) fn node_count(&self) -> usize {
        self.n
    }
    pub(crate) fn metadata_start(&self) -> usize {
        self.metadata_start
    }
    pub(crate) fn descriptor_digest(&self) -> &[u8; 32] {
        self.descriptor
    }
}
pub(crate) fn encode_prefix(instance: &Instance) -> Result<Vec<u8>, CodecError> {
    let mut out = Vec::new();
    append(&mut out, instance.descriptor_digest())?;
    append(&mut out, instance.topology_order_digest())?;
    append(&mut out, instance.nonce().unwrap_or(&[0; 32]))?;
    append(
        &mut out,
        &u32::try_from(instance.node_ids().len())
            .map_err(|_| CodecError::BoundsExceeded)?
            .to_le_bytes(),
    )?;
    append(
        &mut out,
        &u32::try_from(instance.edges().len())
            .map_err(|_| CodecError::BoundsExceeded)?
            .to_le_bytes(),
    )?;
    append(&mut out, &instance.offset_milli().to_le_bytes())?;
    write_column(instance.node_ids(), &mut out)?;
    let mut endpoints = Vec::new();
    endpoints
        .try_reserve_exact(
            instance
                .edges()
                .len()
                .checked_mul(2)
                .ok_or(CodecError::BoundsExceeded)?,
        )
        .map_err(|_| CodecError::Allocation)?;
    for &(a, b) in instance.edges() {
        endpoints.extend_from_slice(&[a, b]);
    }
    write_column(&endpoints, &mut out)?;
    write_palette(instance.h(), &mut out)?;
    write_palette(instance.j(), &mut out)?;
    Ok(out)
}

fn canonical_len(n: usize, e: usize, m: usize) -> Result<usize, CodecError> {
    n.checked_mul(8)
        .and_then(|v| e.checked_mul(12).and_then(|w| v.checked_add(w)))
        .and_then(|v| v.checked_add(m))
        .and_then(|v| v.checked_add(113))
        .ok_or(CodecError::BoundsExceeded)
}
pub(crate) fn preflight_prefix<'a>(
    bytes: &'a [u8],
    limits: &InstanceLimits,
) -> Result<InstancePrefix<'a>, CodecError> {
    // Each palette contributes at most eight bytes per coefficient. Metadata compression
    // replaces canonical headers and never expands beyond the canonical metadata cap.
    let cap = (limits.max_nodes() as usize)
        .checked_mul(12)
        .and_then(|n| {
            (limits.max_edges() as usize)
                .checked_mul(16)
                .and_then(|e| n.checked_add(e))
        })
        .and_then(|n| n.checked_add(limits.max_metadata() as usize))
        .and_then(|n| n.checked_add(124))
        .ok_or(CodecError::BoundsExceeded)?;
    if bytes.len() > cap {
        return Err(CodecError::BoundsExceeded);
    }
    let n = u32::from_le_bytes(take(bytes, 96)?) as usize;
    if n > limits.max_nodes() as usize {
        return Err(CodecError::BoundsExceeded);
    }
    let e = u32::from_le_bytes(take(bytes, 100)?) as usize;
    if e > limits.max_edges() as usize {
        return Err(CodecError::BoundsExceeded);
    }
    if i64::from_le_bytes(take(bytes, 104)?).unsigned_abs() > limits.max_offset() {
        return Err(CodecError::BoundsExceeded);
    }
    let head = bytes.get(..112).ok_or(CodecError::Length)?;
    let _ = canonical_len(n, e, limits.max_metadata() as usize)?;
    let mut at = 112;
    let (ids, len) = read_column(bytes.get(at..).ok_or(CodecError::Length)?, n)?;
    at += len;
    let (endpoints, len) = read_column(
        bytes.get(at..).ok_or(CodecError::Length)?,
        e.checked_mul(2).ok_or(CodecError::BoundsExceeded)?,
    )?;
    at += len;
    let (h, len) = palette(bytes.get(at..).ok_or(CodecError::Length)?, n, limits)?;
    at += len;
    let (j, len) = palette(bytes.get(at..).ok_or(CodecError::Length)?, e, limits)?;
    at += len;
    Ok(InstancePrefix {
        head,
        n,
        e,
        ids,
        endpoints,
        h,
        j,
        metadata_start: at,
        descriptor: head
            .get(..32)
            .ok_or(CodecError::Length)?
            .try_into()
            .map_err(|_| CodecError::Length)?,
    })
}
pub(crate) fn decode_prefix(
    prefix: &InstancePrefix<'_>,
    metadata: Vec<u8>,
    limits: &InstanceLimits,
    kinds: &[u16],
) -> Result<Instance, CodecError> {
    if prefix.n > limits.max_nodes() as usize
        || prefix.e > limits.max_edges() as usize
        || metadata.len() > limits.max_metadata() as usize
    {
        return Err(CodecError::BoundsExceeded);
    }
    let _ = canonical_len(prefix.n, prefix.e, metadata.len())?;
    // Structural checks precede minimality, including edge ordering and metadata.
    crate::instance::validate_metadata(&metadata, kinds)
        .map_err(|_| CodecError::InvalidInstance)?;
    let mut previous = None;
    for i in 0..prefix.e {
        let edge = (
            prefix.endpoints.get(2 * i)?,
            prefix.endpoints.get(2 * i + 1)?,
        );
        if edge.0 >= edge.1 || edge.1 as usize >= prefix.n || previous.is_some_and(|p| p >= edge) {
            return Err(CodecError::InvalidInstance);
        }
        previous = Some(edge);
    }
    prefix.ids.minimal()?;
    prefix.endpoints.minimal()?;
    prefix.h.minimal()?;
    prefix.j.minimal()?;
    let mut node_ids = filled(prefix.n, 0)?;
    for (i, id) in node_ids.iter_mut().enumerate() {
        *id = prefix.ids.get(i)?;
    }
    let mut edges = filled(prefix.e, (0, 0))?;
    for (i, edge) in edges.iter_mut().enumerate() {
        *edge = (
            prefix.endpoints.get(i * 2)?,
            prefix.endpoints.get(i * 2 + 1)?,
        );
    }
    Instance::new(
        InstanceParts {
            descriptor_digest: *prefix.descriptor,
            topology_order_digest: take(prefix.head, 32)?,
            nonce: Some(take(prefix.head, 64)?),
            node_ids,
            edges,
            h: prefix.h.decode()?,
            j: prefix.j.decode()?,
            offset_milli: i64::from_le_bytes(take(prefix.head, 104)?),
            metadata,
        },
        limits,
        kinds,
        None,
    )
    .map_err(|e| {
        if e == crate::Code::BoundsExceeded {
            CodecError::BoundsExceeded
        } else {
            CodecError::InvalidInstance
        }
    })
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test assertions must fail independently of setup errors"
)]
mod tests {
    use super::*;
    #[test]
    fn borrowed_column_defers_minimality() {
        let (column, used) = read_column(&[1, 0], 1).expect("valid wire");
        assert_eq!(used, 2);
        assert_eq!(column.get(0), Ok(0));
        assert_eq!(column.minimal(), Err(CodecError::NonCanonical));
    }

    #[test]
    fn column_widths_and_padding() -> Result<(), CodecError> {
        for values in [&[][..], &[0, 0], &[0, 1], &[0, 1 << 30], &[0, u32::MAX]] {
            let mut bytes = Vec::new();
            write_column(values, &mut bytes)?;
            let (column, len) = read_column(&bytes, values.len())?;
            assert_eq!(len, bytes.len());
            for (i, &v) in values.iter().enumerate() {
                assert_eq!(column.get(i)?, v);
            }
        }
        assert!(matches!(
            read_column(&[1, 0], 1).and_then(|(col, _)| col.minimal()),
            Err(CodecError::NonCanonical)
        ));
        assert!(matches!(
            read_column(&[1, 129], 1),
            Err(CodecError::Padding)
        ));
        assert!(matches!(
            read_column(&[32], usize::MAX),
            Err(CodecError::BoundsExceeded)
        ));
        assert!(matches!(
            read_column(&[33], 0),
            Err(CodecError::InvalidSymbol)
        ));
        Ok(())
    }
    #[test]
    fn fixed_prefix_columns() -> Result<(), CodecError> {
        let bytes = encode_prefix(&synthetic(0)?)?;
        // Independent oracle bytes for node IDs, endpoints, fields, and couplings.
        assert_eq!(
            bytes.get(112..),
            Some(
                &[
                    7, 218, 129, 17, 0, 2, 212, 3, 0, 0, 0, 0, 0, 0, 192, 0, 0, 0, 0, 0, 0, 0, 64,
                    2, 25, 1, 0, 0, 0, 249, 255, 255, 255, 0
                ][..]
            )
        );
        Ok(())
    }
    #[test]
    fn zero_width_hostile_counts() -> Result<(), CodecError> {
        let mut hostile = alloc::vec![0;124];
        hostile
            .get_mut(96..100)
            .ok_or(CodecError::Length)?
            .copy_from_slice(&5u32.to_le_bytes());
        let small = InstanceLimits::new(4, 2, 10, 0, 2).ok_or(CodecError::InvalidInstance)?;
        assert!(matches!(
            preflight_prefix(&hostile, &small),
            Err(CodecError::BoundsExceeded)
        ));
        assert!(matches!(
            preflight_prefix(hostile.get(..100).ok_or(CodecError::Length)?, &small),
            Err(CodecError::BoundsExceeded)
        ));
        Ok(())
    }
    fn synthetic(offset: i64) -> Result<Instance, CodecError> {
        Instance::new(
            InstanceParts {
                descriptor_digest: [1; 32],
                topology_order_digest: [2; 32],
                nonce: None,
                node_ids: alloc::vec![90, 3, 70, 0],
                edges: alloc::vec![(0, 1), (1, 3)],
                h: alloc::vec![0, 1 << 30, 0, -(1 << 30)],
                j: alloc::vec![-7, -7],
                offset_milli: offset,
                metadata: alloc::vec![0, 0],
            },
            &InstanceLimits::FAM_37,
            &[],
            None,
        )
        .map_err(|_| CodecError::InvalidInstance)
    }
    #[test]
    fn prefix_reconstruction() -> Result<(), CodecError> {
        for offset in [-(1 << 56), 0, 1 << 56] {
            let instance = synthetic(offset)?;
            let bytes = encode_prefix(&instance)?;
            let prefix = preflight_prefix(&bytes, &InstanceLimits::FAM_37)?;
            assert_eq!(prefix.node_count(), 4);
            assert_eq!(prefix.e, 2);
            assert_eq!(prefix.metadata_start(), bytes.len());
            assert_eq!(prefix.descriptor_digest(), &[1; 32]);
            let decoded = decode_prefix(&prefix, alloc::vec![0, 0], &InstanceLimits::FAM_37, &[])?;
            assert_eq!(decoded.nonce(), Some(&[0; 32]));
            assert_eq!(
                instance
                    .canonical_bytes()
                    .map_err(|_| CodecError::Allocation)?,
                decoded
                    .canonical_bytes()
                    .map_err(|_| CodecError::Allocation)?
            );
            assert_eq!(instance.digest(), decoded.digest());
            assert_eq!(encode_prefix(&decoded)?, bytes);
        }
        Ok(())
    }
    #[test]
    fn preflight_bounds_and_palette_minimality() -> Result<(), CodecError> {
        let mut bytes = encode_prefix(&synthetic(0)?)?;
        bytes
            .get_mut(96..100)
            .ok_or(CodecError::Length)?
            .copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            preflight_prefix(&bytes, &InstanceLimits::FAM_37),
            Err(CodecError::BoundsExceeded)
        ));
        let limits =
            InstanceLimits::new(4, 2, 1 << 30, 1 << 56, 2).ok_or(CodecError::InvalidInstance)?;
        let bytes = encode_prefix(&synthetic(0)?)?;
        let prefix = preflight_prefix(&bytes, &limits)?;
        assert_eq!(
            decode_prefix(&prefix, alloc::vec![0;3], &limits, &[]),
            Err(CodecError::BoundsExceeded)
        );
        // Palette [0,1] with two zero ranks leaves coefficient 1 unused.
        let palette_bytes = [2, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0];
        let (p, _) = palette(&palette_bytes, 2, &limits)?;
        assert_eq!(p.minimal(), Err(CodecError::NonCanonical));
        assert!(matches!(
            palette(&palette_bytes[..8], 2, &limits),
            Err(CodecError::Length)
        ));
        assert!(matches!(
            palette(&[0, 0, 0, 0, 0], 1, &limits),
            Err(CodecError::InvalidSymbol)
        ));
        let (p, _) = palette(&[0, 0, 0, 0, 0], 0, &limits)?;
        p.minimal()?;
        let mut out = Vec::new();
        write_palette(&[i32::MIN, i32::MAX, 0, i32::MIN], &mut out)?;
        // Full signed values are representable by the primitive but exceed protocol limits.
        assert!(matches!(
            palette(&out, 4, &limits),
            Err(CodecError::BoundsExceeded)
        ));
        let (p, _) = palette(&[1, 0, 0, 0, 0, 0, 0, 0, 1, 0], 2, &limits)?;
        assert_eq!(p.minimal(), Err(CodecError::NonCanonical));
        Ok(())
    }
}

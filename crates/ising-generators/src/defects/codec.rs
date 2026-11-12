//! Lossless defects metadata and pinned-forest witness codecs.
use super::{Config, LIMITS};
use crate::{
    codec::{
        bits::{BitReader, BitWriter},
        instance::{
            decode_prefix, encode_prefix, preflight_prefix, read_column, write_column, Column,
        },
        CodecError, WitnessCodec,
    },
    descriptor::Params,
    ice::build::{sections, Reader},
    instance::Instance,
    native::codec::ForestCodec,
};
use alloc::vec::Vec;

pub(crate) fn prepare(
    params: &Params<'_>,
    instance: &Instance,
) -> Result<WitnessCodec, CodecError> {
    // Registry preparation already validates the complete family model.
    let c = config(params)?;
    let m = metadata(c, instance)?;
    let lattice = crate::square::SquareLattice::new(c.native.side, crate::square::Boundary::Open)
        .map_err(|_| CodecError::InvalidInstance)?;
    let mut labels = reserve(m.labels.len())?;
    for &q in m.labels {
        labels.push(if q == 127 {
            None
        } else {
            Some(i8::from_le_bytes([q]))
        });
    }
    let mut positions = reserve(c.native.links())?;
    positions.extend(values(
        m.incidence.get(4..).ok_or(CodecError::InvalidInstance)?,
    ));
    let mut pins = reserve(m.pins.len() / 4)?;
    for pin in values(m.pins) {
        pins.push((pin, 1));
    }
    Ok(ForestCodec::new(&lattice, &labels, &positions, &pins)?.into())
}

fn take<'a>(r: &mut Reader<'a>, n: usize) -> Result<&'a [u8], CodecError> {
    r.take(n).ok_or(CodecError::Length)
}
fn u16le(r: &mut Reader<'_>) -> Result<u16, CodecError> {
    r.u16().ok_or(CodecError::Length)
}
fn reserve<T>(n: usize) -> Result<Vec<T>, CodecError> {
    let mut v = Vec::new();
    v.try_reserve_exact(n).map_err(|_| CodecError::Allocation)?;
    Ok(v)
}
fn config(params: &Params<'_>) -> Result<Config, CodecError> {
    let c = Config::of(params).ok_or(CodecError::InvalidInstance)?;
    if !c.fits() || c.native.links() > LIMITS.max_nodes() as usize {
        return Err(CodecError::InvalidInstance);
    }
    Ok(c)
}
fn window(c: Config, c0: u16, r0: u16) -> bool {
    let Some(grid) = c.native.grid_side() else {
        return false;
    };
    c0 >= 1
        && r0 >= 1
        && c0.checked_add(c.native.side).is_some_and(|v| v <= grid)
        && r0.checked_add(c.native.side).is_some_and(|v| v <= grid)
}
fn border(side: usize, v: usize) -> bool {
    let w = side + 1;
    let (x, y) = (v % w, v / w);
    x == 0 || y == 0 || x == side || y == side
}
fn permutation(
    values: impl Iterator<Item = Result<u32, CodecError>>,
    n: usize,
) -> Result<(), CodecError> {
    // The frozen defects contract has at most 24 nodes.
    let mut seen = 0u32;
    for value in values {
        let v = value?;
        if v as usize >= n || seen & (1 << v) != 0 {
            return Err(CodecError::InvalidInstance);
        }
        seen |= 1 << v;
    }
    Ok(())
}
fn list<'a>(r: &mut Reader<'a>, bound: usize) -> Result<&'a [u8], CodecError> {
    let n = usize::from(u16le(r)?);
    if n > bound {
        return Err(CodecError::BoundsExceeded);
    }
    take(r, n * 4)
}
fn values(bytes: &[u8]) -> impl Iterator<Item = u32> + '_ {
    bytes.chunks_exact(4).map(|b| {
        let mut a = [0; 4];
        a.copy_from_slice(b);
        u32::from_le_bytes(a)
    })
}
fn check_ranges(
    values: impl Iterator<Item = Result<u32, CodecError>>,
    bound: usize,
) -> Result<(), CodecError> {
    for v in values {
        if v? as usize >= bound {
            return Err(CodecError::InvalidInstance);
        }
    }
    Ok(())
}
struct Metadata<'a> {
    labels: &'a [u8],
    incidence: &'a [u8],
    fixed: &'a [u8],
    obstacles: &'a [u8],
    pins: &'a [u8],
}
fn metadata(c: Config, instance: &Instance) -> Result<Metadata<'_>, CodecError> {
    if instance.node_ids().len() != c.native.links() {
        return Err(CodecError::InvalidInstance);
    }
    let [charges, incidence, defects] =
        sections(instance.metadata(), [4, 5, 8]).ok_or(CodecError::InvalidInstance)?;
    let side = usize::from(c.native.side);
    let vertices = (side + 1) * (side + 1);
    let mut r = Reader::new(charges);
    if u16le(&mut r)? != c.native.side {
        return Err(CodecError::InvalidInstance);
    }
    let labels = take(&mut r, vertices)?;
    if !r.rest().is_empty() {
        return Err(CodecError::InvalidInstance);
    }
    for (v, &q) in labels.iter().enumerate() {
        let good = if border(side, v) {
            q == 127
        } else {
            q == 254 || q == 0 || q == 2
        };
        if !good {
            return Err(CodecError::InvalidInstance);
        }
    }
    let mut r = Reader::new(incidence);
    if !window(c, u16le(&mut r)?, u16le(&mut r)?) {
        return Err(CodecError::InvalidInstance);
    }
    let positions = take(&mut r, c.native.links() * 4)?;
    if !r.rest().is_empty() {
        return Err(CodecError::InvalidInstance);
    }
    permutation(values(positions).map(Ok), c.native.links())?;
    let mut r = Reader::new(defects);
    let fixed = take(&mut r, 48)?;
    let obstacles = list(&mut r, vertices)?;
    let pins = list(&mut r, c.native.links())?;
    if !r.rest().is_empty() {
        return Err(CodecError::InvalidInstance);
    }
    check_ranges(values(obstacles).map(Ok), vertices)?;
    check_ranges(values(pins).map(Ok), c.native.links())?;
    Ok(Metadata {
        labels,
        incidence,
        fixed,
        obstacles,
        pins,
    })
}
pub(crate) fn encode_instance(
    params: &Params<'_>,
    instance: &Instance,
) -> Result<Vec<u8>, CodecError> {
    let c = config(params)?;
    let m = metadata(c, instance).map_err(|e| {
        if e == CodecError::Allocation {
            e
        } else {
            CodecError::UnencodableInstance
        }
    })?;
    let mut out = encode_prefix(instance)?;
    out.try_reserve_exact(
        usize::try_from(LIMITS.max_metadata()).map_err(|_| CodecError::BoundsExceeded)?,
    )
    .map_err(|_| CodecError::Allocation)?;
    out.extend_from_slice(
        m.incidence
            .get(..4)
            .ok_or(CodecError::UnencodableInstance)?,
    );
    let mut bits = BitWriter::new(c.native.interior() * 2)?;
    let side = usize::from(c.native.side);
    for (v, &q) in m.labels.iter().enumerate() {
        if !border(side, v) {
            bits.push(
                match q {
                    254 => 0,
                    0 => 1,
                    2 => 2,
                    _ => return Err(CodecError::UnencodableInstance),
                },
                2,
            )?;
        }
    }
    out.extend_from_slice(&bits.finish()?);
    let mut positions = reserve(c.native.links())?;
    positions.extend(values(
        m.incidence
            .get(4..)
            .ok_or(CodecError::UnencodableInstance)?,
    ));
    write_column(&positions, &mut out)?;
    out.extend_from_slice(m.fixed);
    for bytes in [m.obstacles, m.pins] {
        out.extend_from_slice(
            &u16::try_from(bytes.len() / 4)
                .map_err(|_| CodecError::BoundsExceeded)?
                .to_le_bytes(),
        );
        let mut column = reserve(bytes.len() / 4)?;
        column.extend(values(bytes));
        write_column(&column, &mut out)?;
    }
    Ok(out)
}
fn column<'a>(r: &mut Reader<'a>, count: usize) -> Result<Column<'a>, CodecError> {
    let (column, used) = read_column(r.rest(), count)?;
    let _ = take(r, used)?;
    Ok(column)
}
fn counted_column<'a>(r: &mut Reader<'a>, bound: usize) -> Result<Column<'a>, CodecError> {
    let n = usize::from(u16le(r)?);
    if n > bound {
        return Err(CodecError::BoundsExceeded);
    }
    column(r, n)
}
fn append_column(out: &mut Vec<u8>, col: &Column<'_>) -> Result<(), CodecError> {
    for i in 0..col.count {
        out.extend_from_slice(&col.get(i)?.to_le_bytes());
    }
    Ok(())
}
fn section(out: &mut Vec<u8>, kind: u16, payload: &[u8]) -> Result<(), CodecError> {
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(
        &u32::try_from(payload.len())
            .map_err(|_| CodecError::BoundsExceeded)?
            .to_le_bytes(),
    );
    out.extend_from_slice(payload);
    Ok(())
}
pub(crate) fn decode_instance(
    params: &Params<'_>,
    descriptor_digest: &[u8; 32],
    bytes: &[u8],
) -> Result<Instance, CodecError> {
    let c = config(params)?;
    let prefix = preflight_prefix(bytes, &LIMITS)?;
    let side = usize::from(c.native.side);
    let vertices = (side + 1) * (side + 1);
    let n = c.native.links();
    let mut r = Reader::new(
        bytes
            .get(prefix.metadata_start()..)
            .ok_or(CodecError::Length)?,
    );
    let origin = take(&mut r, 4)?;
    let mut origin_reader = Reader::new(origin);
    let c0 = u16le(&mut origin_reader)?;
    let r0 = u16le(&mut origin_reader)?;
    let charge_bytes = take(&mut r, (c.native.interior() * 2).div_ceil(8))?;
    let mut charges = BitReader::new(charge_bytes, c.native.interior() * 2)?;
    for _ in 0..c.native.interior() {
        if charges.read(2)? == 3 {
            return Err(CodecError::InvalidSymbol);
        }
    }
    let positions = column(&mut r, n)?;
    let fixed = take(&mut r, 48)?;
    let obstacles = counted_column(&mut r, vertices)?;
    let pins = counted_column(&mut r, n)?;
    let expanded = 20 + 2 + vertices + 4 + 4 * n + 48 + 4 + 4 * (obstacles.count + pins.count);
    if expanded > LIMITS.max_metadata() as usize {
        return Err(CodecError::BoundsExceeded);
    }
    if !r.rest().is_empty() {
        return Err(CodecError::Length);
    }
    if prefix.descriptor_digest() != descriptor_digest {
        return Err(CodecError::DescriptorMismatch);
    }
    if prefix.node_count() != n || !window(c, c0, r0) {
        return Err(CodecError::InvalidInstance);
    }
    permutation((0..n).map(|i| positions.get(i)), n)?;
    check_ranges((0..obstacles.count).map(|i| obstacles.get(i)), vertices)?;
    check_ranges((0..pins.count).map(|i| pins.get(i)), n)?;
    let mut metadata = reserve(expanded)?;
    metadata.extend_from_slice(&3u16.to_le_bytes());
    let mut payload = reserve(expanded)?;
    payload.extend_from_slice(&c.native.side.to_le_bytes());
    let mut charges = BitReader::new(charge_bytes, c.native.interior() * 2)?;
    for v in 0..vertices {
        payload.push(if border(side, v) {
            127
        } else {
            match charges.read(2)? {
                0 => 254,
                1 => 0,
                2 => 2,
                _ => return Err(CodecError::InvalidSymbol),
            }
        });
    }
    section(&mut metadata, 4, &payload)?;
    payload.clear();
    payload.extend_from_slice(origin);
    append_column(&mut payload, &positions)?;
    section(&mut metadata, 5, &payload)?;
    payload.clear();
    payload.extend_from_slice(fixed);
    for col in [&obstacles, &pins] {
        payload.extend_from_slice(
            &u16::try_from(col.count)
                .map_err(|_| CodecError::BoundsExceeded)?
                .to_le_bytes(),
        );
        append_column(&mut payload, col)?;
    }
    section(&mut metadata, 8, &payload)?;
    let instance = decode_prefix(&prefix, metadata, &LIMITS, &[4, 5, 8])?;
    for col in [&positions, &obstacles, &pins] {
        col.minimal()?;
    }
    Ok(instance)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::square::{charges, Boundary, SquareLattice};
    #[test]
    #[expect(clippy::indexing_slicing, reason = "fixed test lattice")]
    fn pinned_components_check_roots() {
        let lattice = SquareLattice::new(2, Boundary::Open).expect("lattice");
        let positions: Vec<u32> = (0..12).collect();
        let spins = alloc::vec![1;12];
        let labels: Vec<Option<i8>> = charges(&lattice, &spins)
            .expect("charges")
            .into_iter()
            .map(Some)
            .collect();
        for pins in [
            alloc::vec![],
            lattice.incident(0).iter().map(|&(e, _)| (e, 1)).collect(),
            (0..12).map(|e| (e, 1)).collect(),
        ] {
            let codec = ForestCodec::new(&lattice, &labels, &positions, &pins).expect("forest");
            let bytes = codec.encode(&spins).expect("feasible");
            assert_eq!(codec.decode(&bytes).expect("decode"), spins);
            let mut contradictory = labels.clone();
            contradictory[0] = Some(4);
            let codec =
                ForestCodec::new(&lattice, &contradictory, &positions, &pins).expect("forest");
            assert_eq!(codec.encode(&spins), Err(CodecError::Infeasible));
            assert_eq!(codec.decode(&bytes), Err(CodecError::Infeasible));
        }
    }
}

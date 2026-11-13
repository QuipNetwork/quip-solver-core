//! Prepared square-link forests for hard witness constraints.
use crate::{
    codec::{
        bits::{byte_len, BitReader, BitWriter},
        check_spins, copied, filled, CodecError,
    },
    square::SquareLattice,
};
use alloc::vec::Vec;

pub(crate) struct ForestCodec {
    labels: Vec<Option<i8>>,
    positions: Vec<u32>,
    pins: Vec<Option<i8>>,
    incident: Vec<[(usize, i8); 4]>,
    degrees: Vec<usize>,
    parents: Vec<Option<(usize, i8)>>,
    order: Vec<usize>,
    explicit: Vec<usize>,
    max_len: usize,
}
impl ForestCodec {
    pub(crate) fn new(
        lattice: &SquareLattice,
        labels: &[Option<i8>],
        positions: &[u32],
        pins: &[(u32, i8)],
    ) -> Result<Self, CodecError> {
        let n = lattice.link_count() as usize;
        let v = lattice.vertex_count() as usize;
        if labels.len() != v || positions.len() != n {
            return Err(CodecError::InvalidInstance);
        }
        check_positions(positions, n)?;
        let fixed = fixed_links(n, pins)?;
        let mut incident = filled(v, [(0, 0); 4])?;
        let mut degrees = filled(v, 0)?;
        for vertex in 0..v {
            let vertex_id = u32::try_from(vertex).map_err(|_| CodecError::BoundsExceeded)?;
            let edges = lattice.incident(vertex_id);
            *degrees.get_mut(vertex).ok_or(CodecError::InvalidInstance)? = edges.len();
            for (slot, &(e, s)) in incident
                .get_mut(vertex)
                .ok_or(CodecError::InvalidInstance)?
                .iter_mut()
                .zip(edges)
            {
                *slot = (e as usize, s);
            }
        }
        let mut parents = filled(v, None)?;
        let mut visited = filled(v, false)?;
        let mut parent_edges = filled(n, false)?;
        let mut order = Vec::new();
        order
            .try_reserve_exact(v)
            .map_err(|_| CodecError::Allocation)?;
        for (vertex, label) in labels.iter().enumerate() {
            if label.is_none() {
                *visited.get_mut(vertex).ok_or(CodecError::InvalidInstance)? = true;
                order.push(vertex);
            }
        }
        let mut cursor = 0;
        // Each vertex enters the queue once. The outer scan supplies closed-component roots.
        for root in 0..=v {
            if root != 0 {
                let vertex = root - 1;
                if visited.get(vertex) == Some(&true) {
                    continue;
                }
                *visited.get_mut(vertex).ok_or(CodecError::InvalidInstance)? = true;
                order.push(vertex);
            }
            while let Some(&vertex) = order.get(cursor) {
                cursor += 1;
                for &(e, sign) in
                    lattice.incident(u32::try_from(vertex).map_err(|_| CodecError::BoundsExceeded)?)
                {
                    if fixed
                        .get(e as usize)
                        .ok_or(CodecError::InvalidInstance)?
                        .is_some()
                    {
                        continue;
                    }
                    let link = lattice.link(e).ok_or(CodecError::InvalidInstance)?;
                    let other = if sign == -1 { link.head } else { link.tail } as usize;
                    if visited.get(other) == Some(&true) {
                        continue;
                    }
                    *visited.get_mut(other).ok_or(CodecError::InvalidInstance)? = true;
                    *parents.get_mut(other).ok_or(CodecError::InvalidInstance)? =
                        Some((e as usize, -sign));
                    *parent_edges
                        .get_mut(e as usize)
                        .ok_or(CodecError::InvalidInstance)? = true;
                    order.push(other);
                }
            }
        }
        let mut explicit = Vec::new();
        explicit
            .try_reserve_exact(n)
            .map_err(|_| CodecError::Allocation)?;
        for e in 0..n {
            if fixed.get(e) == Some(&None) && parent_edges.get(e) == Some(&false) {
                explicit.push(e);
            }
        }
        let max_len = byte_len(explicit.len())?;
        Ok(Self {
            labels: copied(labels)?,
            positions: copied(positions)?,
            pins: fixed,
            incident,
            degrees,
            parents,
            order,
            explicit,
            max_len,
        })
    }
    pub(crate) fn spin_count(&self) -> usize {
        self.positions.len()
    }
    pub(crate) fn max_encoded_len(&self) -> usize {
        self.max_len
    }
    fn sum(&self, vertex: usize, spins: &[i8]) -> Result<i16, CodecError> {
        let degree = *self
            .degrees
            .get(vertex)
            .ok_or(CodecError::InvalidInstance)?;
        let mut sum = 0;
        for &(e, s) in self
            .incident
            .get(vertex)
            .and_then(|a| a.get(..degree))
            .ok_or(CodecError::InvalidInstance)?
        {
            let p = *self.positions.get(e).ok_or(CodecError::InvalidInstance)? as usize;
            sum += i16::from(s) * i16::from(*spins.get(p).ok_or(CodecError::InvalidInstance)?);
        }
        Ok(sum)
    }
    pub(crate) fn encode(&self, spins: &[i8]) -> Result<Vec<u8>, CodecError> {
        check_spins(spins, self.spin_count())?;
        for (e, pin) in self.pins.iter().enumerate() {
            if let Some(s) = pin {
                if spins.get(*self.positions.get(e).ok_or(CodecError::InvalidInstance)? as usize)
                    != Some(s)
                {
                    return Err(CodecError::Infeasible);
                }
            }
        }
        for (v, label) in self.labels.iter().enumerate() {
            if let Some(q) = label {
                if self.sum(v, spins)? != i16::from(*q) {
                    return Err(CodecError::Infeasible);
                }
            }
        }
        let mut writer = BitWriter::new(self.explicit.len())?;
        for &e in &self.explicit {
            let p = *self.positions.get(e).ok_or(CodecError::InvalidInstance)? as usize;
            writer.push(u8::from(spins.get(p) == Some(&1)), 1)?;
        }
        writer.finish()
    }
    pub(crate) fn decode(&self, bytes: &[u8]) -> Result<Vec<i8>, CodecError> {
        if bytes.len() > self.max_len {
            return Err(CodecError::Length);
        }
        let mut reader = BitReader::new(bytes, self.explicit.len())?;
        let mut spins = filled(self.spin_count(), 0)?;
        for (&p, pin) in self.positions.iter().zip(&self.pins) {
            if let Some(s) = pin {
                *spins
                    .get_mut(p as usize)
                    .ok_or(CodecError::InvalidInstance)? = *s;
            }
        }
        for &e in &self.explicit {
            let p = *self.positions.get(e).ok_or(CodecError::InvalidInstance)? as usize;
            *spins.get_mut(p).ok_or(CodecError::InvalidInstance)? =
                if reader.read(1)? == 1 { 1 } else { -1 };
        }
        for &v in self.order.iter().rev() {
            let Some(q) = self.labels.get(v).copied().flatten() else {
                continue;
            };
            let residual = i16::from(q) - self.sum(v, &spins)?;
            match self.parents.get(v).copied().flatten() {
                Some((e, sign)) => {
                    let s = residual * i16::from(sign);
                    if s != -1 && s != 1 {
                        return Err(CodecError::Infeasible);
                    }
                    let p = *self.positions.get(e).ok_or(CodecError::InvalidInstance)? as usize;
                    *spins.get_mut(p).ok_or(CodecError::InvalidInstance)? =
                        i8::try_from(s).map_err(|_| CodecError::Infeasible)?;
                }
                None => {
                    if residual != 0 {
                        return Err(CodecError::Infeasible);
                    }
                }
            }
        }
        Ok(spins)
    }
}

fn check_positions(positions: &[u32], n: usize) -> Result<(), CodecError> {
    let mut seen_positions = filled(n, false)?;
    for &p in positions {
        let seen = seen_positions
            .get_mut(p as usize)
            .ok_or(CodecError::InvalidInstance)?;
        if *seen {
            return Err(CodecError::InvalidInstance);
        }
        *seen = true;
    }
    Ok(())
}

fn fixed_links(n: usize, pins: &[(u32, i8)]) -> Result<Vec<Option<i8>>, CodecError> {
    let mut fixed = filled(n, None)?;
    for &(e, s) in pins {
        let slot = fixed
            .get_mut(e as usize)
            .ok_or(CodecError::InvalidInstance)?;
        if slot.is_some() || (s != -1 && s != 1) {
            return Err(CodecError::InvalidInstance);
        }
        *slot = Some(s);
    }
    Ok(fixed)
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test assertions must fail independently of setup errors"
)]
mod tests {
    use super::*;
    use crate::square::{charges, Boundary};
    #[test]
    fn disconnected_roots_and_instance_order() -> Result<(), CodecError> {
        let lattice =
            SquareLattice::new(2, Boundary::Open).map_err(|_| CodecError::InvalidInstance)?;
        let spins = [1; 12];
        let labels: Vec<_> = charges(&lattice, &spins)
            .ok_or(CodecError::InvalidInstance)?
            .into_iter()
            .map(Some)
            .collect();
        let positions: Vec<_> = (0..12).rev().collect();
        let pins: Vec<_> = (0..12).map(|e| (e, 1)).collect();
        let c = ForestCodec::new(&lattice, &labels, &positions, &pins)?;
        assert_eq!(c.encode(&spins)?, []);
        assert_eq!(c.decode(&[])?, spins);
        let mut wrong = labels.clone();
        *wrong.first_mut().ok_or(CodecError::InvalidInstance)? = Some(0);
        assert_eq!(
            ForestCodec::new(&lattice, &wrong, &positions, &pins)?.decode(&[]),
            Err(CodecError::Infeasible)
        );
        let c = ForestCodec::new(&lattice, &labels, &positions, &[])?;
        assert_eq!(c.decode(&c.encode(&spins)?)?, spins);
        Ok(())
    }
    #[test]
    fn exhaustive_free_boundary() -> Result<(), CodecError> {
        let lattice =
            SquareLattice::new(2, Boundary::Open).map_err(|_| CodecError::InvalidInstance)?;
        let mut labels = [None; 9];
        labels[4] = Some(0);
        let positions: Vec<_> = (0..12).rev().collect();
        let c = ForestCodec::new(&lattice, &labels, &positions, &[])?;
        assert_eq!(c.explicit.len(), 11);
        for mask in 0..4096 {
            let spins: Vec<_> = (0..12)
                .map(|i| if mask & (1 << i) != 0 { 1 } else { -1 })
                .collect();
            match c.encode(&spins) {
                Ok(bytes) => assert_eq!(c.decode(&bytes)?, spins),
                Err(CodecError::Infeasible) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

//! Quantized dipolar coupling tables for axis-aligned square ice, and the
//! exact integer validator for them.
//!
//! An island sits at an integer point, has an easy axis along `+x`, `-x`, `+y`,
//! or `-y`, and has an integer moment. For islands `i` and `j` with offset
//! `r = p_j - p_i`, `K = r . r`, and the table constant `D`, the coupling is
//! `D * m_i * m_j * N / K^(5/2)` with `N = K (u_i . u_j) - 3 (u_i . r)(u_j . r)`.
//! The table stores `q = round(S * coupling)` for the integer scale `S`, with
//! ties away from zero.
//!
//! Every check uses integers. The comparison of an integer with a multiple of
//! `K^(5/2)` squares both sides and compares in `u128`, see `cmp_scaled`. The
//! bounds that keep each product below `2^128` are in `docs/dipolar-numerics.md`.
//!
//! A table without a physical calibration is an oracle. [`physical_status`]
//! never returns a success label.

use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;
use core::fmt;

use crate::digest::BufferedHasher;

/// The most islands in a geometry.
pub const MAX_ISLANDS: usize = 8320;
/// The largest island coordinate.
pub const MAX_COORD: u16 = 256;
/// The largest island moment in absolute value.
pub const MAX_MOMENT: u8 = 16;
/// The largest value of `S * |D|`.
pub const MAX_SCALE_PRODUCT: u64 = 1 << 20;
/// The largest truncation radius squared, `2 * 256^2`.
pub const MAX_R2: u32 = 2 * 256 * 256;
/// The longest text field of a calibration record, in bytes.
pub const MAX_TEXT: usize = 256;

/// The derive-key context of the geometry digest.
const GEOMETRY_CONTEXT: &str = "quip dipolar geometry v1";
/// The derive-key context of the table commitment.
const TABLE_CONTEXT: &str = "quip dipolar table v1";
/// Fractional bits of the integer square root used to estimate a coefficient.
const ROOT_BITS: u32 = 30;
/// The most correction steps from the estimate to the exact coefficient.
const MAX_STEPS: u32 = 4;

/// The easy axis of an island.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// Along `+x`.
    PlusX,
    /// Along `-x`.
    MinusX,
    /// Along `+y`.
    PlusY,
    /// Along `-y`.
    MinusY,
}

impl Axis {
    /// The unit vector of the axis.
    const fn vector(self) -> (i64, i64) {
        match self {
            Self::PlusX => (1, 0),
            Self::MinusX => (-1, 0),
            Self::PlusY => (0, 1),
            Self::MinusY => (0, -1),
        }
    }

    /// The byte that the digest uses for the axis.
    const fn code(self) -> u8 {
        match self {
            Self::PlusX => 0,
            Self::MinusX => 1,
            Self::PlusY => 2,
            Self::MinusY => 3,
        }
    }
}

/// One island.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Island {
    /// The x coordinate, at most 256.
    pub x: u16,
    /// The y coordinate, at most 256.
    pub y: u16,
    /// The easy axis.
    pub axis: Axis,
    /// The moment, at most 16 in absolute value.
    pub moment: i8,
}

/// A geometry or a table is not admitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DipolarError {
    /// More than 8320 islands.
    IslandCount,
    /// A coordinate is above 256.
    Coordinate,
    /// A moment is above 16 in absolute value.
    Moment,
    /// The scale `S` is zero.
    Scale,
    /// The constant `D` is zero.
    Constant,
    /// `S * |D|` is above `2^20`.
    ScaleProduct,
    /// `R2` is above `2 * 256^2`.
    Radius,
    /// Two islands share a point.
    CoincidentIslands,
    /// An integer step overflowed. The admitted bounds make this unreachable.
    Arithmetic,
    /// The estimate of a coefficient did not settle in a few steps.
    QuantizeStalled,
    /// An entry names an island that does not exist.
    IndexOutOfRange,
    /// An entry has `i > j`, so it mirrors a pair.
    AsymmetricPair {
        /// The first index.
        i: u32,
        /// The second index.
        j: u32,
    },
    /// An entry pairs an island with itself.
    SelfPair {
        /// The island.
        i: u32,
    },
    /// An entry does not come strictly after the previous entry.
    NotAscending {
        /// The first index.
        i: u32,
        /// The second index.
        j: u32,
    },
    /// An entry lies outside the truncation radius.
    ExtraPair {
        /// The first index.
        i: u32,
        /// The second index.
        j: u32,
    },
    /// A pair inside the radius has no entry.
    MissingPair {
        /// The first index.
        i: u32,
        /// The second index.
        j: u32,
    },
    /// An entry is not the rounded coupling.
    WrongValue {
        /// The first index.
        i: u32,
        /// The second index.
        j: u32,
    },
    /// A calibration text field is empty or longer than 256 bytes.
    CalibrationText,
}

impl fmt::Display for DipolarError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IslandCount => f.write_str("more than 8320 islands"),
            Self::Coordinate => f.write_str("a coordinate is above 256"),
            Self::Moment => f.write_str("a moment is above 16 in absolute value"),
            Self::Scale => f.write_str("the scale is zero"),
            Self::Constant => f.write_str("the constant is zero"),
            Self::ScaleProduct => f.write_str("the scale times the constant is above 2^20"),
            Self::Radius => f.write_str("the radius squared is above 131072"),
            Self::CoincidentIslands => f.write_str("two islands share a point"),
            Self::Arithmetic => f.write_str("an integer step overflowed"),
            Self::QuantizeStalled => f.write_str("the coefficient estimate did not settle"),
            Self::IndexOutOfRange => f.write_str("an entry names an island that does not exist"),
            Self::AsymmetricPair { i, j } => write!(f, "entry ({i}, {j}) mirrors a pair"),
            Self::SelfPair { i } => write!(f, "entry ({i}, {i}) pairs an island with itself"),
            Self::NotAscending { i, j } => write!(f, "entry ({i}, {j}) is out of order"),
            Self::ExtraPair { i, j } => write!(f, "entry ({i}, {j}) is outside the radius"),
            Self::MissingPair { i, j } => write!(f, "pair ({i}, {j}) has no entry"),
            Self::WrongValue { i, j } => write!(f, "entry ({i}, {j}) is not the rounded coupling"),
            Self::CalibrationText => f.write_str("a calibration text is empty or too long"),
        }
    }
}

impl core::error::Error for DipolarError {}

/// Islands and the table constants. The fields are private, so every value
/// passed the bounds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Geometry {
    islands: Vec<Island>,
    d: i32,
    scale: u32,
    r2: u32,
}

impl Geometry {
    /// Checks the bounds and builds the geometry.
    ///
    /// # Errors
    ///
    /// The first failed bound, in the order of the variants of
    /// [`DipolarError`]. Two islands at one point fail with
    /// [`DipolarError::CoincidentIslands`].
    pub fn new(islands: Vec<Island>, d: i32, scale: u32, r2: u32) -> Result<Self, DipolarError> {
        if islands.len() > MAX_ISLANDS {
            return Err(DipolarError::IslandCount);
        }
        if islands.iter().any(|p| p.x > MAX_COORD || p.y > MAX_COORD) {
            return Err(DipolarError::Coordinate);
        }
        if islands.iter().any(|p| p.moment.unsigned_abs() > MAX_MOMENT) {
            return Err(DipolarError::Moment);
        }
        if scale == 0 {
            return Err(DipolarError::Scale);
        }
        if d == 0 {
            return Err(DipolarError::Constant);
        }
        if u64::from(scale) * u64::from(d.unsigned_abs()) > MAX_SCALE_PRODUCT {
            return Err(DipolarError::ScaleProduct);
        }
        if r2 > MAX_R2 {
            return Err(DipolarError::Radius);
        }
        let mut points: Vec<(u16, u16)> = islands.iter().map(|p| (p.x, p.y)).collect();
        points.sort_unstable();
        if points.windows(2).any(|w| w.first() == w.get(1)) {
            return Err(DipolarError::CoincidentIslands);
        }
        Ok(Self {
            islands,
            d,
            scale,
            r2,
        })
    }

    /// The islands, in index order.
    #[must_use]
    pub fn islands(&self) -> &[Island] {
        &self.islands
    }

    /// The table constant `D`.
    #[must_use]
    pub const fn d(&self) -> i32 {
        self.d
    }

    /// The integer scale `S`.
    #[must_use]
    pub const fn scale(&self) -> u32 {
        self.scale
    }

    /// The truncation radius squared.
    #[must_use]
    pub const fn r2(&self) -> u32 {
        self.r2
    }

    /// The BLAKE3 digest of the canonical geometry bytes: the island count,
    /// each island as `x`, `y`, axis code, and moment, then `D`, `S`, and `R2`.
    /// Integers are little endian.
    #[must_use]
    pub fn digest(&self) -> [u8; 32] {
        let mut h = BufferedHasher::new(GEOMETRY_CONTEXT);
        h.put(
            &u32::try_from(self.islands.len())
                .unwrap_or(u32::MAX)
                .to_le_bytes(),
        );
        for p in &self.islands {
            h.put(&p.x.to_le_bytes());
            h.put(&p.y.to_le_bytes());
            h.put(&[p.axis.code()]);
            h.put(&p.moment.to_le_bytes());
        }
        h.put(&self.d.to_le_bytes());
        h.put(&self.scale.to_le_bytes());
        h.put(&self.r2.to_le_bytes());
        h.finish()
    }

    /// The integer `x = S * D * m_i * m_j * N` and the squared distance `K` of
    /// the pair `(i, j)`. `None` for a missing island or an overflow.
    fn pair(&self, i: u32, j: u32) -> Option<(i64, u32)> {
        let first = self.islands.get(i as usize)?;
        let second = self.islands.get(j as usize)?;
        // The offsets and the axis products are at most 2^17 in size.
        let dx = i64::from(second.x) - i64::from(first.x);
        let dy = i64::from(second.y) - i64::from(first.y);
        let dist2 = dx * dx + dy * dy;
        let (ax, ay) = first.axis.vector();
        let (bx, by) = second.axis.vector();
        let normal = dist2 * (ax * bx + ay * by) - 3 * (ax * dx + ay * dy) * (bx * dx + by * dy);
        let numer = i64::from(self.scale)
            .checked_mul(i64::from(self.d))?
            .checked_mul(i64::from(first.moment))?
            .checked_mul(i64::from(second.moment))?
            .checked_mul(normal)?;
        let dist2 = u32::try_from(dist2).ok().filter(|&dist2| dist2 > 0)?;
        Some((numer, dist2))
    }
}

/// One table entry. `i < j`, and entries ascend by `(i, j)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The first island.
    pub i: u32,
    /// The second island.
    pub j: u32,
    /// The quantized coupling.
    pub q: i32,
}

/// The source of a calibrated table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalibrationRecord {
    /// The material, at most 256 bytes and not empty.
    pub material: String,
    /// The lattice spacing in picometers.
    pub lattice_spacing_pm: u32,
    /// How `D` and the moments are normalized, at most 256 bytes and not empty.
    pub normalization: String,
    /// The approximation error of the model in parts per million.
    pub approximation_error_ppm: u32,
    /// The digest of the source document.
    pub source_digest: [u8; 32],
}

/// Whether a table has a physical calibration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Calibration {
    /// A fixed test table. It is not a device model.
    Oracle,
    /// A table with a calibration record.
    Recorded(CalibrationRecord),
}

/// What a table may claim about physics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PhysicalStatus {
    /// The table is an oracle. It supports no physical claim.
    InsufficientPhysicalEvidence,
    /// The table has a record that no reviewer approved.
    RecordedUnreviewed,
}

/// A validated table: geometry, entries, and calibration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    geometry: Geometry,
    entries: Vec<Entry>,
    calibration: Calibration,
    truncation_bound: u128,
}

impl Table {
    /// Validates the entries against the geometry and builds the table.
    ///
    /// # Errors
    ///
    /// The error of [`validate`], or [`DipolarError::CalibrationText`] for a
    /// record with an empty or long text field.
    pub fn new(
        geometry: Geometry,
        entries: Vec<Entry>,
        calibration: Calibration,
    ) -> Result<Self, DipolarError> {
        if let Calibration::Recorded(r) = &calibration {
            let ok = |s: &str| !s.is_empty() && s.len() <= MAX_TEXT;
            if !ok(&r.material) || !ok(&r.normalization) {
                return Err(DipolarError::CalibrationText);
            }
        }
        validate(&geometry, &entries)?;
        let truncation_bound = truncation_bound(&geometry)?;
        Ok(Self {
            geometry,
            entries,
            calibration,
            truncation_bound,
        })
    }

    /// The geometry.
    #[must_use]
    pub const fn geometry(&self) -> &Geometry {
        &self.geometry
    }

    /// The entries.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The calibration.
    #[must_use]
    pub const fn calibration(&self) -> &Calibration {
        &self.calibration
    }

    /// The BLAKE3 commitment: the geometry digest, the entry count and each
    /// entry as `i`, `j`, `q`, then the calibration. Integers are little
    /// endian. Text fields carry a `u32` length. An oracle is tag 0 and a
    /// record is tag 1 followed by the material, the spacing, the
    /// normalization, the error, and the source digest.
    #[must_use]
    pub fn commitment(&self) -> [u8; 32] {
        let mut h = BufferedHasher::new(TABLE_CONTEXT);
        h.put(&self.geometry.digest());
        h.put(
            &u64::try_from(self.entries.len())
                .unwrap_or(u64::MAX)
                .to_le_bytes(),
        );
        for e in &self.entries {
            h.put(&e.i.to_le_bytes());
            h.put(&e.j.to_le_bytes());
            h.put(&e.q.to_le_bytes());
        }
        match &self.calibration {
            Calibration::Oracle => h.put(&[0]),
            Calibration::Recorded(r) => {
                h.put(&[1]);
                for text in [&r.material, &r.normalization] {
                    h.put(&u32::try_from(text.len()).unwrap_or(u32::MAX).to_le_bytes());
                    h.put(text.as_bytes());
                }
                h.put(&r.lattice_spacing_pm.to_le_bytes());
                h.put(&r.approximation_error_ppm.to_le_bytes());
                h.put(&r.source_digest);
            }
        }
        h.finish()
    }

    /// The bound on the rounding error in half units of `q`: one half unit
    /// for each entry, so the count of entries.
    #[must_use]
    pub fn rounding_bound(&self) -> u128 {
        self.entries.len() as u128
    }

    /// The bound on the omitted magnitude in units of `q`, see
    /// [`truncation_bound`].
    #[must_use]
    pub const fn truncation_bound(&self) -> u128 {
        self.truncation_bound
    }
}

/// An upper bound on the magnitude that the table omits. It sums, over every
/// pair with `K > R2`, the integer `ceil(|x| / (K^2 * isqrt(K)))`. Since
/// `isqrt(K) <= sqrt(K)`, each term is at least `S * |coupling|` for that pair.
///
/// # Errors
///
/// [`DipolarError::Arithmetic`] on an overflow, which the bounds exclude.
pub fn truncation_bound(geometry: &Geometry) -> Result<u128, DipolarError> {
    let n = u32::try_from(geometry.islands.len()).map_err(|_| DipolarError::IslandCount)?;
    let mut total: u128 = 0;
    for i in 0..n {
        for j in i.saturating_add(1)..n {
            let (x, k) = geometry.pair(i, j).ok_or(DipolarError::Arithmetic)?;
            if k <= geometry.r2 {
                continue;
            }
            let den = u128::from(k) * u128::from(k) * u128::from(k.isqrt());
            let term = u128::from(x.unsigned_abs()).div_ceil(den);
            total = total.checked_add(term).ok_or(DipolarError::Arithmetic)?;
        }
    }
    Ok(total)
}

/// Compares the integer `b` with `a * K^2 * sqrt(K)` exactly.
///
/// The signs decide first. For equal signs the magnitudes compare as `b^2`
/// against `a^2 * K^5` in `u128`, mirrored for negative values. A product that
/// overflows `u128` is at least `2^128`, which exceeds `b^2` because
/// `b^2 < 2^128`, so the overflow saturates the right side.
fn cmp_scaled(b: i64, a: i64, k: u32) -> Ordering {
    let sign_b = b.signum();
    let sign_a = a.signum();
    if sign_b != sign_a {
        return sign_b.cmp(&sign_a);
    }
    if sign_a == 0 {
        return Ordering::Equal;
    }
    let lhs = u128::from(b.unsigned_abs()) * u128::from(b.unsigned_abs());
    let rhs = u128::from(k)
        .checked_pow(5)
        .and_then(|k5| {
            (u128::from(a.unsigned_abs()) * u128::from(a.unsigned_abs())).checked_mul(k5)
        })
        .unwrap_or(u128::MAX);
    let magnitude = lhs.cmp(&rhs);
    if sign_a < 0 {
        magnitude.reverse()
    } else {
        magnitude
    }
}

/// How the correct rounding of `x / K^(5/2)` relates to `q`: `Less` when the
/// correct value is below `q`, `Greater` when above, `Equal` when `q` is
/// correct. Ties go away from zero, so the lower bound is inclusive for
/// `q > 0` and the upper bound is inclusive for `q < 0`.
fn judge(x: i64, q: i64, k: u32) -> Result<Ordering, DipolarError> {
    let b = x.checked_mul(2).ok_or(DipolarError::Arithmetic)?;
    let lo = cmp_scaled(b, 2 * q - 1, k);
    let hi = cmp_scaled(b, 2 * q + 1, k);
    let below = if q > 0 {
        lo == Ordering::Less
    } else {
        lo != Ordering::Greater
    };
    let above = if q >= 0 {
        hi != Ordering::Less
    } else {
        hi == Ordering::Greater
    };
    Ok(if below {
        Ordering::Less
    } else if above {
        Ordering::Greater
    } else {
        Ordering::Equal
    })
}

/// The exact `q` for `x` and `K`, found from an integer estimate.
fn quantize_pair(x: i64, k: u32) -> Result<i32, DipolarError> {
    // root = floor(sqrt(K) * 2^30), so the estimate is within one of the answer.
    let root = (u128::from(k) << (2 * ROOT_BITS)).isqrt();
    let den = u128::from(k) * u128::from(k) * root;
    let magnitude = (u128::from(x.unsigned_abs()) << ROOT_BITS)
        .checked_div(den)
        .ok_or(DipolarError::Arithmetic)?;
    let magnitude = i64::try_from(magnitude).map_err(|_| DipolarError::Arithmetic)?;
    let mut q = if x < 0 { -magnitude } else { magnitude };
    for _ in 0..=MAX_STEPS {
        match judge(x, q, k)? {
            Ordering::Equal => return i32::try_from(q).map_err(|_| DipolarError::Arithmetic),
            Ordering::Less => q -= 1,
            Ordering::Greater => q += 1,
        }
    }
    Err(DipolarError::QuantizeStalled)
}

/// Every pair inside the radius with its exact `q`, in ascending `(i, j)`
/// order.
///
/// # Errors
///
/// [`DipolarError::Arithmetic`] or [`DipolarError::QuantizeStalled`], which
/// the bounds exclude.
pub fn quantize(geometry: &Geometry) -> Result<Vec<Entry>, DipolarError> {
    let n = u32::try_from(geometry.islands.len()).map_err(|_| DipolarError::IslandCount)?;
    let mut entries = Vec::new();
    for i in 0..n {
        for j in i.saturating_add(1)..n {
            let (x, k) = geometry.pair(i, j).ok_or(DipolarError::Arithmetic)?;
            if k <= geometry.r2 {
                entries.push(Entry {
                    i,
                    j,
                    q: quantize_pair(x, k)?,
                });
            }
        }
    }
    Ok(entries)
}

/// The next pair at or after `cursor` that lies inside the radius.
fn next_in_radius(
    geometry: &Geometry,
    cursor: &mut (u32, u32),
) -> Result<Option<(u32, u32)>, DipolarError> {
    let n = geometry.islands.len();
    loop {
        if (cursor.0 as usize).saturating_add(1) >= n {
            return Ok(None);
        }
        if cursor.1 as usize >= n {
            cursor.0 = cursor.0.saturating_add(1);
            cursor.1 = cursor.0.saturating_add(1);
            continue;
        }
        let (_, k) = geometry
            .pair(cursor.0, cursor.1)
            .ok_or(DipolarError::Arithmetic)?;
        if k <= geometry.r2 {
            return Ok(Some(*cursor));
        }
        cursor.1 = cursor.1.saturating_add(1);
    }
}

/// Checks the entries against the geometry with integers only.
///
/// The entries must be strictly ascending with `i < j`. They must hold every
/// pair with `K <= R2` and no other pair. Each `q` must be the rounded
/// coupling, with ties away from zero.
///
/// # Errors
///
/// The first fault in entry order. A pair inside the radius that no entry
/// covers is reported before the entry that follows it. A pair missing after
/// the last entry is reported at the end.
pub fn validate(geometry: &Geometry, entries: &[Entry]) -> Result<(), DipolarError> {
    let count = geometry.islands.len();
    let mut cursor = (0_u32, 1_u32);
    let mut previous: Option<(u32, u32)> = None;
    for entry in entries {
        let (i, j) = (entry.i, entry.j);
        if i as usize >= count || j as usize >= count {
            return Err(DipolarError::IndexOutOfRange);
        }
        if i > j {
            return Err(DipolarError::AsymmetricPair { i, j });
        }
        if i == j {
            return Err(DipolarError::SelfPair { i });
        }
        if previous.is_some_and(|p| (i, j) <= p) {
            return Err(DipolarError::NotAscending { i, j });
        }
        previous = Some((i, j));
        let (numer, dist2) = geometry.pair(i, j).ok_or(DipolarError::Arithmetic)?;
        if let Some((mi, mj)) = next_in_radius(geometry, &mut cursor)? {
            if (mi, mj) < (i, j) {
                return Err(DipolarError::MissingPair { i: mi, j: mj });
            }
        }
        if dist2 > geometry.r2 {
            return Err(DipolarError::ExtraPair { i, j });
        }
        if judge(numer, i64::from(entry.q), dist2)? != Ordering::Equal {
            return Err(DipolarError::WrongValue { i, j });
        }
        cursor.1 = cursor.1.saturating_add(1);
    }
    match next_in_radius(geometry, &mut cursor)? {
        Some((i, j)) => Err(DipolarError::MissingPair { i, j }),
        None => Ok(()),
    }
}

/// The physical status of a table. No result is a validation success: a
/// reviewer approval is out of scope.
#[must_use]
pub const fn physical_status(table: &Table) -> PhysicalStatus {
    match table.calibration {
        Calibration::Oracle => PhysicalStatus::InsufficientPhysicalEvidence,
        Calibration::Recorded(_) => PhysicalStatus::RecordedUnreviewed,
    }
}

#[cfg(test)]
mod tests {
    use super::{cmp_scaled, Ordering, ROOT_BITS};

    /// Compares by bracketing `sqrt(K)` between two fixed-point integers, a
    /// method that shares no code with `cmp_scaled`.
    fn brute(b: i64, a: i64, k: u32) -> Ordering {
        let scale = 1_i128 << ROOT_BITS;
        let k = i128::from(k);
        let root = (k << (2 * ROOT_BITS)).isqrt();
        let exact = root * root == k << (2 * ROOT_BITS);
        let left = i128::from(b) * scale;
        let unit = i128::from(a) * k * k;
        let low = unit * root;
        let high = unit * (root + i128::from(!exact));
        let (low, high) = if low <= high {
            (low, high)
        } else {
            (high, low)
        };
        if left < low {
            Ordering::Less
        } else if left > high {
            Ordering::Greater
        } else {
            assert!(exact || low == high, "ambiguous case b={b} a={a} k={k}");
            if exact {
                left.cmp(&low)
            } else {
                // a == 0 here, since the bracket is a single point.
                left.cmp(&0)
            }
        }
    }

    #[test]
    fn cmp_scaled_matches_a_bracketing_method() {
        for k in 1..=40_u32 {
            for a in -9..=9_i64 {
                for b in -3000..=3000_i64 {
                    assert_eq!(cmp_scaled(b, a, k), brute(b, a, k), "b={b} a={a} k={k}");
                }
            }
        }
    }

    #[test]
    fn cmp_scaled_saturates_without_overflow() {
        assert_eq!(cmp_scaled(i64::MAX, i64::MAX, u32::MAX), Ordering::Less);
        assert_eq!(cmp_scaled(i64::MIN, i64::MIN, u32::MAX), Ordering::Greater);
        assert_eq!(cmp_scaled(i64::MAX, i64::MIN, u32::MAX), Ordering::Greater);
    }
}

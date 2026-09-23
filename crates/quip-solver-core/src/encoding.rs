//! Validated coefficient decoding and per-problem exactness.

use crate::coefficient::{Coefficient, WireForm};
use quip_proto::v1::{CoefficientEncoding, IsingProblem, RejectReason};

type MilliPair = (Vec<i32>, Vec<i32>);

pub(crate) struct Decoded<C> {
    pub(crate) h: Vec<C>,
    pub(crate) j: Vec<C>,
    pub(crate) exact_milli: Option<MilliPair>,
}

/// Convert milli-valued fields and couplings to the chosen coefficient type.
///
/// Returns converted fields, converted couplings, and the original milli vectors
/// when conversion loses precision. Conversion uses [`Coefficient::from_milli`],
/// including its rounding and saturation behavior.
#[must_use]
#[expect(
    clippy::float_cmp,
    reason = "exactness requires equality, not a tolerance"
)]
pub fn convert_milli<C: Coefficient>(
    h: Vec<i32>,
    j: Vec<i32>,
) -> (Vec<C>, Vec<C>, Option<MilliPair>) {
    let mut exact = true;
    let mut convert = |values: &[i32]| {
        values
            .iter()
            .map(|&m| {
                let value = C::from_milli(m);
                exact &= C::EXACT || value.to_unit() * 1000.0 == f64::from(m);
                value
            })
            .collect()
    };
    let converted_h = convert(&h);
    let converted_j = convert(&j);
    (converted_h, converted_j, (!exact).then_some((h, j)))
}

fn integer_milli(value: i128, scale: u32) -> Result<i32, RejectReason> {
    let product = value * 1000;
    let scale = i128::from(scale);
    if product % scale != 0 {
        return Err(RejectReason::Malformed);
    }
    i32::try_from(product / scale).map_err(|_| RejectReason::Malformed)
}

#[expect(
    clippy::float_cmp,
    reason = "wire milli values must be whole numbers exactly"
)]
pub(crate) fn float_milli(value: f64) -> Result<i32, RejectReason> {
    let milli = value * 1000.0;
    if !milli.is_finite()
        || milli.round() != milli
        || milli < f64::from(i32::MIN)
        || milli > f64::from(i32::MAX)
    {
        return Err(RejectReason::Malformed);
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "validated integral value within i32 range"
    )]
    Ok(milli as i32)
}

fn array<const N: usize>(bytes: &[u8]) -> Result<[u8; N], RejectReason> {
    bytes.try_into().map_err(|_| RejectReason::Malformed)
}

fn stored_milli(
    encoding: CoefficientEncoding,
    scale: u32,
    bytes: &[u8],
) -> Result<i32, RejectReason> {
    match encoding {
        CoefficientEncoding::I8 => {
            integer_milli(i128::from(i8::from_le_bytes(array(bytes)?)), scale)
        }
        CoefficientEncoding::I16 => {
            integer_milli(i128::from(i16::from_le_bytes(array(bytes)?)), scale)
        }
        CoefficientEncoding::I32 => {
            integer_milli(i128::from(i32::from_le_bytes(array(bytes)?)), scale)
        }
        CoefficientEncoding::F16 => float_milli(half::f16::from_le_bytes(array(bytes)?).to_f64()),
        CoefficientEncoding::F32 => float_milli(f64::from(f32::from_le_bytes(array(bytes)?))),
        CoefficientEncoding::F64 => float_milli(f64::from_le_bytes(array(bytes)?)),
        CoefficientEncoding::Unspecified => Err(RejectReason::Malformed),
    }
}

pub(crate) fn decode_problem<C: Coefficient>(
    ising: &IsingProblem,
) -> Result<Decoded<C>, RejectReason> {
    let encoding =
        CoefficientEncoding::try_from(ising.encoding).map_err(|_| RejectReason::Malformed)?;
    let (width, form) = match encoding {
        CoefficientEncoding::I8 | CoefficientEncoding::I16 | CoefficientEncoding::I32 => {
            if ising.scale == 0 {
                return Err(RejectReason::Malformed);
            }
            let width = match encoding {
                CoefficientEncoding::I8 => 1,
                CoefficientEncoding::I16 => 2,
                _ => 4,
            };
            (
                width,
                WireForm::Int {
                    encoding,
                    scale: ising.scale,
                },
            )
        }
        CoefficientEncoding::F16 | CoefficientEncoding::F32 | CoefficientEncoding::F64 => {
            if ising.scale != 0 {
                return Err(RejectReason::Malformed);
            }
            let width = match encoding {
                CoefficientEncoding::F16 => 2,
                CoefficientEncoding::F32 => 4,
                _ => 8,
            };
            (width, WireForm::Float(encoding))
        }
        CoefficientEncoding::Unspecified => return Err(RejectReason::Malformed),
    };
    if !ising.h.len().is_multiple_of(width) || !ising.j.len().is_multiple_of(width) {
        return Err(RejectReason::Malformed);
    }
    if C::WIRE_FORM == form {
        let decode = |bytes: &[u8]| {
            bytes
                .chunks_exact(width)
                .map(|chunk| {
                    let _ = stored_milli(encoding, ising.scale, chunk)?;
                    Ok(C::from_wire_le(chunk))
                })
                .collect::<Result<Vec<C>, RejectReason>>()
        };
        return Ok(Decoded {
            h: decode(&ising.h)?,
            j: decode(&ising.j)?,
            exact_milli: None,
        });
    }
    let decode = |bytes: &[u8]| {
        bytes
            .chunks_exact(width)
            .map(|chunk| stored_milli(encoding, ising.scale, chunk))
            .collect::<Result<Vec<_>, _>>()
    };
    let (h, j, exact_milli) = convert_milli(decode(&ising.h)?, decode(&ising.j)?);
    Ok(Decoded { h, j, exact_milli })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coefficient::Fixed;
    use quip_proto::v1::{CoefficientEncoding, IsingProblem, RejectReason};

    fn problem(encoding: CoefficientEncoding, scale: u32, bytes: Vec<u8>) -> IsingProblem {
        IsingProblem {
            encoding: encoding as i32,
            scale,
            h: bytes.clone(),
            j: bytes,
            ..Default::default()
        }
    }

    macro_rules! round_trip {
        ($name:ident, $encoding:ident, $scale:expr, $values:expr) => {
            #[test]
            fn $name() {
                let bytes = $values.into_iter().flat_map(|v| v.to_le_bytes()).collect();
                let decoded =
                    decode_problem::<f64>(&problem(CoefficientEncoding::$encoding, $scale, bytes))
                        .unwrap();
                assert_eq!(decoded.h, vec![-1.0, 0.0, 1.0]);
                assert_eq!(decoded.j, decoded.h);
                assert!(decoded.exact_milli.is_none());
            }
        };
    }
    round_trip!(round_trip_i8, I8, 1, [-1_i8, 0, 1]);
    round_trip!(round_trip_i16, I16, 1000, [-1000_i16, 0, 1000]);
    round_trip!(round_trip_i32, I32, 1000, [-1000_i32, 0, 1000]);
    round_trip!(
        round_trip_f16,
        F16,
        0,
        [-1.0, 0.0, 1.0].map(half::f16::from_f64)
    );
    round_trip!(round_trip_f32, F32, 0, [-1_f32, 0.0, 1.0]);
    round_trip!(round_trip_f64, F64, 0, [-1_f64, 0.0, 1.0]);

    #[test]
    fn direct_read_i8() {
        let decoded =
            decode_problem::<Fixed<i8, 1>>(&problem(CoefficientEncoding::I8, 1, vec![1, 0xFF, 0]))
                .unwrap();
        assert_eq!(decoded.h, vec![Fixed(1), Fixed(-1), Fixed(0)]);
        assert!(decoded.exact_milli.is_none());
    }

    macro_rules! malformed {
        ($name:ident, $encoding:ident, $scale:expr, $bytes:expr) => {
            #[test]
            fn $name() {
                assert_eq!(
                    decode_problem::<f64>(&problem(CoefficientEncoding::$encoding, $scale, $bytes))
                        .err(),
                    Some(RejectReason::Malformed)
                );
            }
        };
    }
    malformed!(unspecified, Unspecified, 1000, vec![]);
    malformed!(zero_integer_scale, I8, 0, vec![1]);
    malformed!(nonzero_float_scale, F32, 5, 1_f32.to_le_bytes().to_vec());
    malformed!(incomplete_element, I16, 1, vec![0; 3]);
    malformed!(fractional_integer_milli, I8, 3, vec![1]);
    malformed!(nan, F32, 0, f32::NAN.to_le_bytes().to_vec());
    malformed!(
        fractional_float_milli,
        F64,
        0,
        0.0005_f64.to_le_bytes().to_vec()
    );
    malformed!(integer_overflow, I32, 1, i32::MAX.to_le_bytes().to_vec());
    malformed!(
        float_overflow,
        F64,
        0,
        2_147_483.648_f64.to_le_bytes().to_vec()
    );

    #[test]
    fn production_f16_is_exact() {
        let bytes = [-1000_i32, 0, 1000]
            .into_iter()
            .flat_map(i32::to_le_bytes)
            .collect();
        let decoded =
            decode_problem::<half::f16>(&problem(CoefficientEncoding::I32, 1000, bytes)).unwrap();
        assert!(decoded.exact_milli.is_none());
    }
    #[test]
    fn fractional_f16_retains_milli() {
        let decoded = decode_problem::<half::f16>(&problem(
            CoefficientEncoding::I32,
            1000,
            100_i32.to_le_bytes().to_vec(),
        ))
        .unwrap();
        assert_eq!(decoded.exact_milli, Some((vec![100], vec![100])));
    }
    #[test]
    fn direct_float_still_validates() {
        assert_eq!(
            decode_problem::<f32>(&problem(
                CoefficientEncoding::F32,
                0,
                f32::NAN.to_le_bytes().to_vec()
            ))
            .err(),
            Some(RejectReason::Malformed)
        );
    }
}

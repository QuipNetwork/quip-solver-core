//! Coefficient representations and conversions from wire milli values.

mod sealed {
    pub trait Sealed {}
}

/// A supported coefficient representation, sealed to this crate.
pub trait Coefficient: sealed::Sealed + Copy + Send + Sync + 'static {
    /// Whether every wire coefficient survives conversion without loss.
    const EXACT: bool;
    /// Convert wire milli, rounding halfway values away from zero and saturating integers.
    fn from_milli(milli: i32) -> Self;
    /// Convert units. Non-finite input returns `None` for fixed-point types.
    fn from_unit(unit: f64) -> Option<Self>;
    /// Return the value in units for temperature and upload calculations.
    fn to_unit(self) -> f64;
}

impl sealed::Sealed for f64 {}
impl Coefficient for f64 {
    const EXACT: bool = true;
    fn from_milli(milli: i32) -> Self {
        Self::from(milli) / 1000.0
    }
    fn from_unit(unit: f64) -> Option<Self> {
        Some(unit)
    }
    fn to_unit(self) -> f64 {
        self
    }
}

impl sealed::Sealed for f32 {}
impl Coefficient for f32 {
    const EXACT: bool = false;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "representation deliberately narrows the f64 quotient"
    )]
    fn from_milli(milli: i32) -> Self {
        (f64::from(milli) / 1000.0) as Self
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "representation deliberately narrows unit values"
    )]
    fn from_unit(unit: f64) -> Option<Self> {
        Some(unit as Self)
    }
    fn to_unit(self) -> f64 {
        f64::from(self)
    }
}

impl sealed::Sealed for half::f16 {}
impl Coefficient for half::f16 {
    const EXACT: bool = false;
    fn from_milli(milli: i32) -> Self {
        Self::from_f64(f64::from(milli) / 1000.0)
    }
    fn from_unit(unit: f64) -> Option<Self> {
        Some(Self::from_f64(unit))
    }
    fn to_unit(self) -> f64 {
        self.to_f64()
    }
}

/// A fixed-point coefficient storing units multiplied by `SCALE`.
///
/// `SCALE` must be positive. Conversions at scale zero fail to compile.
///
/// ```compile_fail
/// use quip_solver_core::coefficient::{Coefficient, Fixed};
/// const INVALID_SCALE: bool = Fixed::<i8, 0>::EXACT;
/// let value = Fixed::<i8, 0>::from_milli(1);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct Fixed<T, const SCALE: u32>(
    /// Stored integer, equal to units multiplied by `SCALE`.
    pub T,
);

/// Exact wire milli coefficients.
pub type Milli = Fixed<i32, 1000>;

/// A signed four-bit integer stored in one byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct I4(i8);

impl I4 {
    /// Smallest stored value.
    pub const MIN: i8 = -8;
    /// Largest stored value.
    pub const MAX: i8 = 7;
    /// Construct a value in `-8..=7`, or return `None`.
    #[must_use]
    pub const fn new(value: i8) -> Option<Self> {
        if value >= Self::MIN && value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }
    /// Return the stored signed integer.
    #[must_use]
    pub const fn get(self) -> i8 {
        self.0
    }
}

fn rounded_milli<const SCALE: u32>(milli: i32) -> i128 {
    const {
        assert!(SCALE > 0, "SCALE must be positive");
    }
    let product = i128::from(milli) * i128::from(SCALE);
    let magnitude = (product.abs() + 500) / 1000;
    if product < 0 {
        -magnitude
    } else {
        magnitude
    }
}

macro_rules! fixed_integer {
    ($ty:ty, $wire:expr) => {
        impl<const SCALE: u32> sealed::Sealed for Fixed<$ty, SCALE> {}
        impl<const SCALE: u32> Coefficient for Fixed<$ty, SCALE> {
            const EXACT: bool = {
                assert!(SCALE > 0, "SCALE must be positive");
                $wire && SCALE == 1000
            };
            fn from_milli(milli: i32) -> Self {
                let value = rounded_milli::<SCALE>(milli)
                    .clamp(i128::from(<$ty>::MIN), i128::from(<$ty>::MAX));
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "clamped to the destination integer range"
                )]
                Self(value as $ty)
            }
            fn from_unit(unit: f64) -> Option<Self> {
                const {
                    assert!(SCALE > 0, "SCALE must be positive");
                }
                if !unit.is_finite() {
                    return None;
                }
                // Rust float-to-integer casts saturate, including product overflow.
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "round first, then saturate to the destination range"
                )]
                Some(Self((unit * f64::from(SCALE)).round() as $ty))
            }
            fn to_unit(self) -> f64 {
                const {
                    assert!(SCALE > 0, "SCALE must be positive");
                }
                f64::from(self.0) / f64::from(SCALE)
            }
        }
    };
}
fixed_integer!(i32, true);
fixed_integer!(i16, false);
fixed_integer!(i8, false);

impl<const SCALE: u32> sealed::Sealed for Fixed<I4, SCALE> {}
impl<const SCALE: u32> Coefficient for Fixed<I4, SCALE> {
    const EXACT: bool = {
        assert!(SCALE > 0, "SCALE must be positive");
        false
    };
    fn from_milli(milli: i32) -> Self {
        let value = rounded_milli::<SCALE>(milli).clamp(i128::from(I4::MIN), i128::from(I4::MAX));
        #[expect(
            clippy::cast_possible_truncation,
            reason = "clamped to the four-bit signed range"
        )]
        Self(I4(value as i8))
    }
    fn from_unit(unit: f64) -> Option<Self> {
        const {
            assert!(SCALE > 0, "SCALE must be positive");
        }
        if !unit.is_finite() {
            return None;
        }
        let value = (unit * f64::from(SCALE))
            .round()
            .clamp(f64::from(I4::MIN), f64::from(I4::MAX));
        #[expect(
            clippy::cast_possible_truncation,
            reason = "rounded and clamped to the four-bit signed range"
        )]
        Some(Self(I4(value as i8)))
    }
    fn to_unit(self) -> f64 {
        const {
            assert!(SCALE > 0, "SCALE must be positive");
        }
        f64::from(self.0.get()) / f64::from(SCALE)
    }
}

//! Coefficient conversion boundaries and declared exactness.
use quip_solver_core::coefficient::{Coefficient, Fixed, Milli, I4};

const INPUTS: [i32; 13] = [
    0,
    1,
    -1,
    499,
    -499,
    500,
    -500,
    501,
    -501,
    i32::MIN,
    i32::MAX,
    1500,
    -1500,
];

#[test]
fn integer_tables() {
    let whole32 = [0, 0, 0, 0, 0, 1, -1, 1, -1, -2_147_484, 2_147_484, 2, -2];
    let whole16 = [0, 0, 0, 0, 0, 1, -1, 1, -1, -32768, 32767, 2, -2];
    let whole8 = [0, 0, 0, 0, 0, 1, -1, 1, -1, -128, 127, 2, -2];
    let whole4 = [0, 0, 0, 0, 0, 1, -1, 1, -1, -8, 7, 2, -2];
    assert_eq!(INPUTS.map(|v| Fixed::<i32, 1>::from_milli(v).0), whole32);
    assert_eq!(INPUTS.map(|v| Fixed::<i16, 1>::from_milli(v).0), whole16);
    assert_eq!(INPUTS.map(|v| Fixed::<i8, 1>::from_milli(v).0), whole8);
    assert_eq!(
        INPUTS.map(|v| Fixed::<I4, 1>::from_milli(v).0.get()),
        whole4
    );
    assert_eq!(INPUTS.map(|v| Milli::from_milli(v).0), INPUTS);
    assert_eq!(Fixed::<i32, 3>::from_milli(500).0, 2);
    assert_eq!(Fixed::<i32, 3>::from_milli(-500).0, -2);
    assert_eq!(Fixed::<i32, { u32::MAX }>::from_milli(i32::MAX).0, i32::MAX);
    assert_eq!(Fixed::<i32, { u32::MAX }>::from_milli(i32::MIN).0, i32::MIN);
}

#[test]
fn float_tables() {
    let units = [
        0.0_f64,
        0.001,
        -0.001,
        0.499,
        -0.499,
        0.5,
        -0.5,
        0.501,
        -0.501,
        -2_147_483.648,
        2_147_483.647,
        1.5,
        -1.5,
    ];
    let f32_bits = [
        0,
        0x3a83_126f,
        0xba83_126f,
        0x3eff_7cee,
        0xbeff_7cee,
        0x3f00_0000,
        0xbf00_0000,
        0x3f00_4189,
        0xbf00_4189,
        0xca03_126f,
        0x4a03_126f,
        0x3fc0_0000,
        0xbfc0_0000,
    ];
    let f16_bits = [
        0, 0x1419, 0x9419, 0x37fc, 0xb7fc, 0x3800, 0xb800, 0x3802, 0xb802, 0xfc00, 0x7c00, 0x3e00,
        0xbe00,
    ];
    assert_eq!(
        INPUTS.map(|v| f64::from_milli(v).to_bits()),
        units.map(f64::to_bits)
    );
    assert_eq!(INPUTS.map(|v| f32::from_milli(v).to_bits()), f32_bits);
    assert_eq!(INPUTS.map(|v| half::f16::from_milli(v).to_bits()), f16_bits);
}

fn check_fixed<C: Coefficient>() {
    for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(C::from_unit(v).is_none());
    }
    assert!(C::from_unit(f64::MAX).is_some());
    assert!(C::from_unit(-f64::MAX).is_some());
}

#[test]
fn finite_saturation_nonfinite_and_exactness() {
    check_fixed::<Milli>();
    check_fixed::<Fixed<i32, 1>>();
    check_fixed::<Fixed<i16, 1>>();
    check_fixed::<Fixed<i8, 1>>();
    check_fixed::<Fixed<I4, 1>>();
    assert_eq!(Fixed::<i8, 1>::from_unit(0.5), Some(Fixed(1)));
    assert_eq!(Fixed::<i8, 1>::from_unit(-0.5), Some(Fixed(-1)));
    assert_eq!(Fixed::<i8, 1>::from_unit(f64::MAX), Some(Fixed(127)));
    assert_eq!(Fixed::<i8, 1>::from_unit(-f64::MAX), Some(Fixed(-128)));
    assert_eq!(Fixed::<i16, 1>::from_unit(f64::MAX), Some(Fixed(i16::MAX)));
    assert_eq!(Fixed::<i16, 1>::from_unit(-f64::MAX), Some(Fixed(i16::MIN)));
    assert_eq!(Milli::from_unit(f64::MAX), Some(Fixed(i32::MAX)));
    assert_eq!(Milli::from_unit(-f64::MAX), Some(Fixed(i32::MIN)));
    assert_eq!(
        Fixed::<I4, 1>::from_unit(f64::MAX).map(|v| v.0.get()),
        Some(7)
    );
    assert_eq!(
        Fixed::<I4, 1>::from_unit(-f64::MAX).map(|v| v.0.get()),
        Some(-8)
    );
    assert_eq!(Fixed::<I4, 1>::from_unit(-0.5).map(|v| v.0.get()), Some(-1));
    assert_eq!(f32::from_unit(0.001).map(f32::to_bits), Some(0x3a83_126f));
    assert_eq!(
        half::f16::from_unit(0.001).map(half::f16::to_bits),
        Some(0x1419)
    );
    assert_eq!(f32::from_milli(500).to_unit().to_bits(), 0.5_f64.to_bits());
    assert_eq!(
        half::f16::from_milli(500).to_unit().to_bits(),
        0.5_f64.to_bits()
    );
    let flags = [
        f64::EXACT,
        Milli::EXACT,
        f32::EXACT,
        half::f16::EXACT,
        Fixed::<i32, 1>::EXACT,
        Fixed::<i16, 1000>::EXACT,
        Fixed::<i8, 1>::EXACT,
        Fixed::<I4, 1>::EXACT,
    ];
    assert_eq!(
        flags,
        [true, true, false, false, false, false, false, false]
    );
    for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.0, 0.0004] {
        assert_eq!(f64::from_unit(v).map(f64::to_bits), Some(v.to_bits()));
    }
    assert_eq!(I4::new(-9), None);
    assert_eq!(I4::new(8), None);
    assert_eq!(I4::new(I4::MIN).map(I4::get), Some(-8));
    assert_eq!(I4::new(I4::MAX).map(I4::get), Some(7));
    assert_eq!(size_of::<I4>(), 1);
    assert_eq!(
        Milli::from_milli(-501).to_unit().to_bits(),
        (-0.501_f64).to_bits()
    );
}

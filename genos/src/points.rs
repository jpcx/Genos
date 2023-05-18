use std::{
    fmt::Display,
    ops::{Add, AddAssign, Sub, SubAssign},
};

use serde::{de, Deserialize, Serialize, Serializer};
use thiserror::Error;

/// Points is an abstraction over a float32 points system upholds invariants that points can only
/// be multiples of 0.25. Internally, points is represented as an integer which has shifted the
/// f32 value two decimal places to the left. This makes sure that any math or equality checks are
/// consistent and don't need to worry about issues around comparing floats.
#[derive(Default, Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub struct Points(u64);

impl Points {
    pub fn new<I: Into<f64>>(points: I) -> Self {
        let points = points.into();
        validate_points_value(points).expect("expected points to be valid");
        Self(Self::transform(points))
    }

    fn transform(points: f64) -> u64 {
        (points * 100.0) as u64
    }

    fn to_f64(&self) -> f64 {
        (self.0 as f64) / 100.0
    }
}

impl Display for Points {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:.2}", self.to_f64())
    }
}

impl From<f64> for Points {
    fn from(value: f64) -> Self {
        Points::new(value)
    }
}

impl From<f32> for Points {
    fn from(value: f32) -> Self {
        Points::new(value as f64)
    }
}

impl From<u64> for Points {
    fn from(value: u64) -> Self {
        Points::new(value as f64)
    }
}

impl From<u32> for Points {
    fn from(value: u32) -> Self {
        Points::new(value as f64)
    }
}

impl From<i32> for Points {
    fn from(value: i32) -> Self {
        Points::new(value as f64)
    }
}

impl From<i64> for Points {
    fn from(value: i64) -> Self {
        Points::new(value as f64)
    }
}

impl From<&Points> for Points {
    fn from(value: &Points) -> Self {
        *value
    }
}

impl Add for Points {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

impl AddAssign for Points {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

impl Sub for Points {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self(self.0 - rhs.0)
    }
}

impl SubAssign for Points {
    fn sub_assign(&mut self, rhs: Self) {
        assert!(self.0 >= rhs.0);
        self.0 -= rhs.0;
    }
}

fn validate_points_value(val: f64) -> Result<(), PointsValidationError> {
    validate_points_multiple_025(val)?;
    validate_points_precision(val)?;
    validate_points_positive(val)?;
    Ok(())
}

fn validate_points_multiple_025(val: f64) -> Result<(), PointsValidationError> {
    let val = Points::transform(val);
    if val % 25 != 0 {
        return Err(PointsValidationError::InvalidMultiple);
    }
    Ok(())
}

fn validate_points_precision(val: f64) -> Result<(), PointsValidationError> {
    let string_val = format!("{}", val);
    if let Some((_, right)) = string_val.split_once(".") {
        if right.len() > 2 {
            return Err(PointsValidationError::InvalidPrecision);
        }
    }
    Ok(())
}

fn validate_points_positive(val: f64) -> Result<(), PointsValidationError> {
    if val < 0.00 {
        return Err(PointsValidationError::NegativePoints);
    }
    Ok(())
}

#[derive(Debug, Error)]
enum PointsValidationError {
    #[error("precision must be less than or equal to 2")]
    InvalidPrecision,

    #[error("points must be a multiple of 0.25")]
    InvalidMultiple,

    #[error("points must not be negative")]
    NegativePoints,
}

impl<'de> Deserialize<'de> for Points {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct PointsVisitor;

        impl<'de> serde::de::Visitor<'de> for PointsVisitor {
            type Value = Points;

            #[inline]
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("Points in int or float format, must be a multiple of 0.25")
            }

            #[inline]
            fn visit_f64<E>(self, v: f64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                validate_points_value(v).map_err(de::Error::custom)?;
                Ok(Points::new(v))
            }

            #[inline]
            fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                validate_points_value(v as f64).map_err(de::Error::custom)?;
                self.visit_f64(v as f64)
            }

            #[inline]
            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                self.visit_f64(v as f64)
            }
        }

        deserializer.deserialize_any(PointsVisitor)
    }
}

impl Serialize for Points {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_f64(self.to_f64())
    }
}

/// Points type is a convenience enum for use in configuration files which could be used to
/// indicate if a stage in a test is worth partial or full points.
#[derive(Debug, Eq, PartialEq, Clone, Copy, Deserialize)]
pub enum PointQuantity {
    FullPoints,
    Partial(Points),
}

impl PointQuantity {
    pub fn zero() -> Self {
        Self::Partial(0.into())
    }

    pub fn is_full_points(&self) -> bool {
        match self {
            Self::FullPoints => true,
            _ => false,
        }
    }
}

impl Display for PointQuantity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FullPoints => write!(f, "fullpoints"),
            Self::Partial(points) => write!(f, "{}", points),
        }
    }
}

impl Add for PointQuantity {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        match rhs {
            Self::FullPoints => Self::FullPoints,
            Self::Partial(rhs_points) => match self {
                Self::FullPoints => Self::FullPoints,
                Self::Partial(curr_points) => Self::Partial(curr_points + rhs_points),
            },
        }
    }
}

impl AddAssign for PointQuantity {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl From<Points> for PointQuantity {
    fn from(value: Points) -> Self {
        Self::Partial(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Serialize, Deserialize)]
    #[allow(dead_code)]
    struct Config {
        inner: Points,
    }

    #[test]
    fn parse_success_float_from_toml() {
        toml::from_str::<Config>(
            r#"
            inner = 0.25
        "#,
        )
        .unwrap();
    }

    #[test]
    fn parse_success_int_from_toml() {
        toml::from_str::<Config>(
            r#"
            inner = 42
        "#,
        )
        .unwrap();
    }

    #[test]
    fn parse_fail_points_incorrect_precision() {
        toml::from_str::<Config>(
            r#"
            inner = 4.255
        "#,
        )
        .unwrap_err();
    }

    #[test]
    fn parse_fail_points_negative() {
        toml::from_str::<Config>(
            r#"
            inner = -1.25
        "#,
        )
        .unwrap_err();
    }

    #[test]
    fn parse_fail_points_not_multiple_025() {
        toml::from_str::<Config>(
            r#"
            inner = 1.33
        "#,
        )
        .unwrap_err();
    }

    #[test]
    fn serialize_points() {
        let config = Config { inner: 42.into() };
        let _toml = toml::to_string(&config).unwrap();
    }

    #[test]
    #[should_panic]
    fn new_points_panic_precision() {
        Points::new(0.254);
    }

    #[test]
    #[should_panic]
    fn new_points_panic_multiple_025() {
        Points::new(4.33);
    }

    #[test]
    #[should_panic]
    fn new_points_panic_negative() {
        Points::new(-5.0);
    }

    #[test]
    fn new_points_from_int() {
        let _points: Points = 23.into();
    }

    #[test]
    fn point_inner_correct() {
        let points = Points::new(4.25);
        assert_eq!(points.0, 425);

        let points = Points::new(0.50);
        assert_eq!(points.0, 50);
    }

    #[test]
    fn adding_points() {
        let res = Points::new(4.0) + Points::new(1.25);
        assert_eq!(res, Points::new(5.25));
    }

    #[test]
    fn subtracting_points() {
        let res = Points::new(6.0) - Points::new(3.0);
        assert_eq!(res, Points::new(3.0));

        let res = Points::new(4.0) - Points::new(4.0);
        assert_eq!(res, Points::default());
    }

    #[test]
    #[should_panic]
    #[allow(unused_must_use)]
    fn subtracting_points_underflow() {
        Points::new(6.0) - Points::new(42.0);
    }
}

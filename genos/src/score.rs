use std::ops::{Add, AddAssign};

use crate::points::Points;

#[derive(Clone, Copy, Default, Debug, Eq, PartialEq)]
pub struct Score {
    received: Points,
    possible: Points,
}

impl Score {
    pub fn new<P: Into<Points>, R: Into<Points>>(received: R, possible: P) -> Self {
        Self {
            received: received.into(),
            possible: possible.into(),
        }
    }

    pub fn empty() -> Score {
        Score::default()
    }

    pub fn zero_points<T: Into<Points>>(max: T) -> Self {
        Self::new(0, max)
    }

    pub fn full_points<T: Into<Points>>(max: T) -> Self {
        let points = max.into();
        Self::new(points, points)
    }

    pub fn possible(&self) -> Points {
        self.possible
    }

    pub fn received(&self) -> Points {
        self.received
    }

    pub fn points_lost(&self) -> Points {
        self.possible - self.received
    }

    pub fn received_full_points(&self) -> bool {
        self.possible == self.received
    }
}

impl Add for Score {
    type Output = Score;

    fn add(self, rhs: Self) -> Self::Output {
        let mut this = self;
        this.received += rhs.received;
        this.possible += rhs.possible;
        this
    }
}

impl AddAssign for Score {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_score() {
        let score = Score::default();
        assert_eq!(score.received, 0.into());
        assert_eq!(score.possible, 0.into());
    }

    #[test]
    fn zero_points() {
        let score = Score::zero_points(12);
        assert_eq!(score.received, 0.into());
        assert_eq!(score.possible, 12.into());
    }

    #[test]
    fn points_lost() {
        let score = Score::new(3, 5);
        assert_eq!(score.points_lost(), 2.into());
    }

    #[test]
    fn add_score() {
        let a = Score::default();
        let b = Score::new(1, 1);
        let c = Score::new(2, 3);

        assert_eq!(a + b, Score::new(1, 1));
        assert_eq!(b + c, Score::new(3, 4));
        assert_eq!(a + b + c, Score::new(3, 4));
    }
}

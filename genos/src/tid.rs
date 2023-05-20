use std::fmt::Display;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct TestId(u32);

impl TestId {
    pub fn new(id: u32) -> Self {
        Self(id)
    }
}

impl Display for TestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u32> for TestId {
    fn from(value: u32) -> Self {
        Self::new(value)
    }
}

impl From<i32> for TestId {
    fn from(value: i32) -> Self {
        assert!(value >= 0);
        Self::new(value as u32)
    }
}

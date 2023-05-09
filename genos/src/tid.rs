use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq, Ord, PartialOrd)]
pub struct TestId(u32);

impl TestId {
    pub fn new(id: u32) -> Self {
        Self(id)
    }
}

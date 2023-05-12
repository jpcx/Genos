use anyhow::Result;
use async_trait::async_trait;
use std::path::Path;

pub mod fs;
pub mod genos;
pub mod gs;
pub mod output;
pub mod points;
pub mod process;
pub mod score;
pub mod stage;
pub mod test;
pub mod test_util;
pub mod tid;
pub mod writer;

#[async_trait]
pub trait Executor: Send + Sync {
    type Output;
    async fn run(&self, ws: &Path) -> Result<Self::Output>;
}

use async_trait::async_trait;

pub struct ResultsList;

#[async_trait]
pub trait Writer {
    async fn write(&self, results: ResultsList);
}

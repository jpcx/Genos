use std::{path::PathBuf, sync::Arc};

use async_trait::async_trait;

use crate::{
    formatter::Formatter,
    gs::{self, Description},
    output::{Output, Section},
    score::Score,
    test::TestStatus,
};
use anyhow::Result;

/// TestOutput contains all the necessary information to report results to gradescope
pub trait TestOutput: Description + Send + Sync {
    fn status(&self) -> TestStatus;
    fn output(&self) -> Output;
}

/// Anything implementing Transform can transform their content using a formatter.
pub trait Transform {
    fn transform<F: Formatter>(&self, fmt: &F) -> String;
}

#[async_trait]
pub trait ResultsWriter: Send + Sync {
    async fn write(&self, results: Vec<Arc<dyn TestOutput>>) -> Result<()>;
}

pub struct StdoutWriter<F> {
    formatter: F,
}

#[async_trait]
impl<F> ResultsWriter for StdoutWriter<F>
where
    F: Formatter + Send + Sync,
{
    async fn write(&self, results: Vec<Arc<dyn TestOutput>>) -> Result<()> {
        let mut score = Score::empty();
        let mut failed = Vec::new();

        for result in &results {
            score += result.status().score();
            if let TestStatus::Fail(_) = result.status() {
                failed.push(result.clone());
            }

            let output = result.output().transform(&self.formatter);
            let gs_status: gs::TestStatus = result.status().into();

            println!("#################################");
            println!("Test Case {}", result.id());
            println!("Name: {}", result.name());
            println!("Visibility: {}", result.visibility());
            println!("Score: {}", result.status().score());
            println!("Display Status: {}", gs_status);
            println!("|---    Gradescope  Output   ---|");
            println!("{}", output);
            println!("#################################");
        }

        println!("Score: {}", score);

        let failed_display = failed
            .iter()
            .map(|res| res.id().to_string())
            .collect::<Vec<_>>()
            .join(", ");

        println!("Failed: {}", failed_display);

        Ok(())
    }
}

pub struct ResultsJsonWriter<F> {
    formatter: F,
    path: PathBuf,
}

#[async_trait]
impl<F> ResultsWriter for ResultsJsonWriter<F>
where
    F: Formatter + gs::FormatType + Send + Sync,
{
    async fn write(&self, results: Vec<Arc<dyn TestOutput>>) -> Result<()> {
        let mut score = Score::empty();
        let mut test_results = Vec::new();

        for result in &results {
            let test_status: gs::TestStatus = result.status().into();

            let section = Section::new("Overview")
                .content((
                    "Score",
                    format!("{} ({})", result.status().score(), test_status),
                ))
                .content(("Description", result.description()));

            let mut output = Output::new().section(section);
            output.append(result.output());

            let test_result = gs::TestResult {
                score: result.status().score().received(),
                max_score: result.status().score().possible(),
                status: result.status().into(),
                name: result.name(),
                output: output.transform(&self.formatter),
                tags: result.tags(),
                visibility: result.visibility(),
            };

            test_results.push(test_result);
            score += result.status().score();
        }

        let output_results = gs::Results {
            output_format: self.formatter.format_type(),
            tests: test_results,
        };

        // need to output the results to a json file at path
        todo!();
    }
}

#[cfg(test)]
mod tests {}

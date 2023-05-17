use std::sync::Arc;

use crate::config::{Cli, Config};

/// Holds all the context required to execute a run of the autograder
pub struct Context {
    cli_config: Arc<Cli>,
    hw_config: Arc<Config>,
}

impl Context {
    pub fn new(cli_config: Cli) -> Self {
        todo!()
    }
}

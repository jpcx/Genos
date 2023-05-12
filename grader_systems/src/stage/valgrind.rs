use std::path::{Path, PathBuf};

//use anyhow::Result;
//use async_trait::async_trait;
use serde::Deserialize;

//use super::SystemStageExecutor;

#[derive(Default)]
pub struct Valgrind {
    options: Vec<String>,
    target: PathBuf,
}

impl Valgrind {
    pub fn new(config: &ValgrindConfig, target: &Path) -> Self {
        let mut valgrind = Valgrind::default();

        if let Some(v) = config.leak_check {
            valgrind.options.push(format!("--leak-check={:?}", v));
        }

        if let Some(v) = config.error_exitcode {
            valgrind.options.push(format!("--error-exitcode={}", v));
        }

        if let Some(v) = &config.log_file {
            valgrind.options.push(format!("--log-file={}", v));
        }

        if let Some(v) = config.malloc_fill {
            valgrind.options.push(format!("--malloc-fill=0x{:02X}", v));
        }

        if let Some(v) = config.free_fill {
            valgrind.options.push(format!("--free-fill=0x{:02X}", v));
        }

        if let Some(v) = &config.suppressions {
            valgrind.options.push(format!("--suppressions={}", v));
        }

        valgrind.target = target.into();

        valgrind
    }
}

//#[async_trait]
//impl SystemStageExecutor for Valgrind {
//    async fn run(&self, ws: &Path) -> Result<()> {
//        Ok(())
//    }
//}

#[derive(Default, Deserialize, Clone)]
pub struct ValgrindConfig {
    leak_check: Option<bool>,
    error_exitcode: Option<i16>,
    log_file: Option<String>,
    malloc_fill: Option<u8>,
    free_fill: Option<u8>,
    suppressions: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ctor_default() {
        let default = Valgrind::new(&ValgrindConfig::default(), Path::new("foo"));
        assert!(default.options.len() == 0);
        assert!(default.target.to_str().unwrap() == "foo");
    }

    #[tokio::test]
    async fn ctor_valid_supp() {
        //        let supp = "\
        //{
        //   SUPRESS_READLINE_ERRORS
        //   Memcheck:Leak
        //   match-leak-kinds: reachable
        //   ...
        //   fun:readline
        //   fun:main
        //}
        //{
        //   SUPRESS_READLINE_INTERNAL_ERRORS
        //   Memcheck:Leak
        //   match-leak-kinds: reachable
        //   ...
        //   fun:readline
        //}";

        let cfg = ValgrindConfig {
            leak_check: Some(true),
            error_exitcode: Some(200),
            log_file: Some("valgrind.log".to_string()),
            malloc_fill: Some(0xFF),
            free_fill: Some(0xC8),
            suppressions: Some("valgrind.supp".to_string()),
        };

        let sample = Valgrind::new(&cfg, Path::new("bar"));
        assert!(sample.options.len() == 6);
        assert!(sample.options[0] == "--leak-check=true");
        assert!(sample.options[1] == "--error-exitcode=200");
        assert!(sample.options[2] == "--log-file=valgrind.log");
        assert!(sample.options[3] == "--malloc-fill=0xFF");
        assert!(sample.options[4] == "--free-fill=0xC8");
        assert!(sample.options[5] == "--suppressions=valgrind.supp");
        assert!(sample.target.to_str().unwrap() == "bar");
    }

    //#[tokio::test]
    //async fn ctor_missing_sup() {
    //    let data = MockDir::new();

    //    let cfg = ValgrindConfig {
    //        leak_check: Some(true),
    //        error_exitcode: Some(200),
    //        log_file: Some("valgrind.log".to_string()),
    //        malloc_fill: Some(0xFF),
    //        free_fill: Some(0xC8),
    //        suppressions: Some("valgrind.supp".to_string()),
    //    };

    //    assert!(Valgrind::new(&cfg, "bar", &data).is_err());
    //}
}

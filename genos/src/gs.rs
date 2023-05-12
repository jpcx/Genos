use std::path::PathBuf;

pub fn running_in_gs() -> bool {
    PathBuf::from("/autograder").exists()
}

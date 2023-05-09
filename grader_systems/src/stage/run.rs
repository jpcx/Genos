use genos::points::PointsType;

pub struct RunConfig {
    args: Vec<String>,
    executable: String,
    disable_garbage_memory: Option<bool>,
    return_code: Option<ReturnCodeConfig>,
}

pub struct ReturnCodeConfig {
    expected: i32,
    points: PointsType,
}

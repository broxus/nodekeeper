use indicatif::{ProgressBar, ProgressStyle};

pub struct Spinner {
    progress_bar: ProgressBar,
}

pub fn spinner_style() -> &'static ProgressStyle {
    once!(ProgressStyle, || {
        ProgressStyle::with_template("{prefix:.bold.dim} {spinner} {wide_msg}")
            .unwrap()
            .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ")
    })
}

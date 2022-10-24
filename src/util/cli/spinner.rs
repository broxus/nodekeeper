use indicatif::{ProgressBar, ProgressStyle};
use once_cell::race::OnceBox;

pub struct Spinner {
    progress_bar: ProgressBar,
}

pub fn spinner_style() -> &'static ProgressStyle {
    static STYLE: OnceBox<ProgressStyle> = OnceBox::new();
    STYLE.get_or_init(|| {
        Box::new(
            ProgressStyle::with_template("{prefix:.bold.dim} {spinner} {wide_msg}")
                .unwrap()
                .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ "),
        )
    })
}

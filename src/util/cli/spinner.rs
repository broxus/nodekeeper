use std::borrow::Cow;
use std::time::Duration;

use indicatif::{ProgressBar, ProgressStyle};

pub struct Spinner {
    progress_bar: ProgressBar,
}

impl Spinner {
    pub fn start<T: Into<Cow<'static, str>>>(message: T) -> Self {
        let progress_bar = ProgressBar::new_spinner()
            .with_style(spinner_style().clone())
            .with_message(message);

        progress_bar.enable_steady_tick(Duration::from_millis(100));

        Self { progress_bar }
    }

    pub fn set_message<T: Into<Cow<'static, str>>>(&self, message: T) {
        self.progress_bar.set_message(message);
    }

    pub fn println<T: AsRef<str>>(&self, text: T) {
        self.progress_bar.println(text);
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.progress_bar.finish_and_clear();
    }
}

pub fn spinner_style() -> &'static ProgressStyle {
    once!(ProgressStyle, || {
        ProgressStyle::with_template("{prefix:.bold.dim} {spinner} {wide_msg}")
            .unwrap()
            .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ")
    })
}

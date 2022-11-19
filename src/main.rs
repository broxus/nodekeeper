use anyhow::Result;

#[macro_export]
macro_rules! once {
    ($ty:path, || $expr:expr) => {{
        static ONCE: once_cell::race::OnceBox<$ty> = once_cell::race::OnceBox::new();
        ONCE.get_or_init(|| Box::new($expr))
    }};
}

macro_rules! selector_variant {
    ($ty:ident, { $($name:ident => $text:literal),*$(,)? }) => {
        #[derive(Copy, Clone, Eq, PartialEq)]
        enum $ty {
            $($name),*,
        }

        impl $ty {
            fn all() -> Vec<Self> {
                vec![$(Self::$name),*]
            }
        }

        impl ::std::fmt::Display for $ty {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(match self {
                    $(Self::$name => $text),*,
                })
            }
        }
    };
}

mod cli;
mod config;
mod contracts;
mod crypto;
mod exporter;
mod node_tcp_rpc;
mod node_udp_rpc;
mod subscription;
mod util;

#[tokio::main]
async fn main() -> Result<()> {
    if console::user_attended() {
        tracing_subscriber::fmt::init();
    } else {
        tracing_subscriber::fmt::fmt().without_time().init();
    }

    argh::from_env::<cli::App>().run().await
}

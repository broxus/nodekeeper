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
            #[allow(unused)]
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
mod dirs;
mod exporter;
mod network;
mod util;
mod validator;

#[tokio::main]
async fn main() -> Result<()> {
    if console::user_attended() {
        tracing_subscriber::fmt::init();
    } else {
        tracing_subscriber::fmt::fmt().without_time().init();
    }

    argh::from_env::<ArgsOrVersion<cli::App>>().0.run().await
}

struct ArgsOrVersion<T: argh::FromArgs>(T);

impl<T: argh::FromArgs> argh::TopLevelCommand for ArgsOrVersion<T> {}

impl<T: argh::FromArgs> argh::FromArgs for ArgsOrVersion<T> {
    fn from_args(command_name: &[&str], args: &[&str]) -> Result<Self, argh::EarlyExit> {
        /// Also use argh for catching `--version`-only invocations
        #[derive(argh::FromArgs)]
        struct Version {
            /// print version information and exit
            #[argh(switch, short = 'v')]
            pub version: bool,
        }

        match Version::from_args(command_name, args) {
            Ok(v) if v.version => Err(argh::EarlyExit {
                output: format!(
                    "{} {}",
                    command_name.first().unwrap_or(&""),
                    env!("CARGO_PKG_VERSION")
                ),
                status: Ok(()),
            }),
            Err(exit) if exit.status.is_ok() => {
                let help = match T::from_args(command_name, &["--help"]) {
                    Ok(_) => unreachable!(),
                    Err(exit) => exit.output,
                };
                Err(argh::EarlyExit {
                    output: format!("{help}  -v, --version     print version information and exit"),
                    status: Ok(()),
                })
            }
            _ => T::from_args(command_name, args).map(Self),
        }
    }
}

use std::future::Future;
use std::io::{Read, Write};
use std::path::Path;
use std::str::FromStr;

use anyhow::{Context, Result};
use dialoguer::console;
use dialoguer::theme::Theme;
use tokio::process::Command;
use ton_block::Deserializable;

pub async fn exec(command: &mut Command) -> Result<()> {
    let mut child = command.spawn()?;

    let status = child
        .wait()
        .await
        .context("child process encountered an error")?;

    anyhow::ensure!(
        status.success(),
        "child process failed with exit code {status}"
    );
    Ok(())
}

pub struct Tokens<T>(pub T);

impl<T: Into<u128> + Copy> std::fmt::Display for Tokens<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let num: u128 = self.0.into();
        let int = num / 1000000000;
        let mut frac = num % 1000000000;

        int.fmt(f)?;
        if frac > 0 {
            while frac % 10 == 0 && frac > 0 {
                frac /= 10;
            }
            f.write_fmt(format_args!(".{frac}"))?;
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct AddressInput(pub ton_block::MsgAddressInt);

impl std::fmt::Display for AddressInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for AddressInput {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ton_block::MsgAddressInt::from_str(s.trim())
            .map(Self)
            .map_err(|_| anyhow::Error::msg("invalid address"))
    }
}

#[derive(Clone)]
pub struct OptionalAddressInput(pub Option<ton_block::MsgAddressInt>);

impl std::fmt::Display for OptionalAddressInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Some(addr) => addr.fmt(f),
            None => Ok(()),
        }
    }
}

impl FromStr for OptionalAddressInput {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Ok(Self(None));
        }

        let AddressInput(addr) = s.parse()?;
        Ok(Self(Some(addr)))
    }
}

pub fn parse_contract_abi<P>(path: P) -> Result<ton_abi::Contract>
where
    P: AsRef<Path>,
{
    let data = std::fs::read(path.as_ref()).context("failed to read JSON ABI")?;
    let mut jd = serde_json::Deserializer::from_slice(&data);
    let contract: ton_abi::contract::SerdeContract =
        serde_path_to_error::deserialize(&mut jd).context("failed to parse JSON ABI")?;
    ton_abi::Contract::try_from(contract)
}

pub fn parse_address(address: &str) -> Result<ton_block::MsgAddressInt> {
    ton_block::MsgAddressInt::from_str(address).map_err(From::from)
}

pub fn parse_optional_pubkey(pubkey: Option<String>) -> Result<Option<ed25519_dalek::PublicKey>> {
    match pubkey {
        Some(pubkey) => {
            let pubkey = parse_hex_or_base64(&pubkey)?;
            Ok(Some(ed25519_dalek::PublicKey::from_bytes(&pubkey)?))
        }
        None => Ok(None),
    }
}

pub fn parse_optional_input(data: Option<String>, raw: bool) -> Result<Vec<u8>> {
    match data {
        Some(data) if raw => Ok(data.into()),
        Some(data) => parse_hex_or_base64(&data),
        None => {
            let mut data = Vec::new();
            std::io::stdin()
                .read_to_end(&mut data)
                .context("failed to read from stdin")?;
            Ok(data)
        }
    }
}

pub fn parse_key_hash(hash: &str) -> Result<[u8; 32]> {
    let data = parse_hex_or_base64(hash).context("invalid key hash")?;
    data.try_into()
        .map_err(|_| anyhow::Error::msg("invalid key hash length"))
}

pub fn parse_optional_state_init(data: Option<String>) -> Result<Option<ton_block::StateInit>> {
    data.as_deref().map(parse_state_init).transpose()
}

pub fn parse_state_init(data: &str) -> Result<ton_block::StateInit> {
    ton_block::StateInit::construct_from_base64(data)
}

pub fn parse_hex_or_base64(data: &str) -> Result<Vec<u8>> {
    if let Some(hash) = data.strip_prefix("0x") {
        hex::decode(hash).map_err(From::from)
    } else {
        match hex::decode(data) {
            Ok(bytes) => Ok(bytes),
            Err(e) => match base64::decode(data) {
                Ok(bytes) => Ok(bytes),
                _ => Err(e.into()),
            },
        }
    }
}

pub fn confirm<T>(theme: &dyn Theme, default: bool, text: T) -> std::io::Result<bool>
where
    T: Into<String>,
{
    dialoguer::Confirm::with_theme(theme)
        .with_prompt(text)
        .default(default)
        .interact()
}

pub fn print_output<T: std::fmt::Display>(arg: T) {
    if is_terminal() {
        writeln!(std::io::stdout(), "{arg:#}")
    } else {
        write!(std::io::stdout(), "{arg}")
    }
    .unwrap()
}

pub fn print_error(text: impl std::fmt::Display) {
    if is_terminal() {
        eprintln!("{}", console::style(format!("✘ {text}")).red().bold());
    } else {
        eprintln!("Error: {text}");
    }
}

pub fn note(text: impl std::fmt::Display) -> impl std::fmt::Display {
    console::style(format!("({text})")).dim()
}

pub struct Steps {
    total: usize,
    current: usize,
}

impl Steps {
    pub fn new(total: usize) -> Self {
        Self { total, current: 0 }
    }

    pub fn next(&mut self, text: impl std::fmt::Display) {
        if is_terminal() {
            eprintln!(
                "{} {text}",
                console::style(format!("[{}/{}]", self.current, self.total))
                    .bold()
                    .dim()
            );
        } else {
            eprintln!("[{}/{}] {text}", self.current, self.total);
        }
        self.current += 1;
    }
}

pub fn is_terminal() -> bool {
    use once_cell::race::OnceBox;

    static IS_TERMINAL: OnceBox<bool> = OnceBox::new();
    *IS_TERMINAL.get_or_init(|| Box::new(console::user_attended()))
}

pub async fn invoke_as_cli<F>(f: F) -> Result<()>
where
    F: Future<Output = Result<()>>,
{
    setup_handlers();
    f.await.or_else(ignore_interrupt)
}

fn setup_handlers() {
    if !is_terminal() {
        return;
    }

    ctrlc::set_handler(|| {
        let term = dialoguer::console::Term::stdout();
        let _ = term.show_cursor();
    })
    .expect("Error setting Ctrl-C handler");
}

fn ignore_interrupt(e: anyhow::Error) -> Result<()> {
    if !is_terminal() {
        return Err(e);
    }

    if let Some(e) = e.downcast_ref::<std::io::Error>() {
        if e.kind() == std::io::ErrorKind::Interrupted {
            eprintln!();
            return Ok(());
        }
    }

    Err(e)
}

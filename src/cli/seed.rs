use anyhow::Result;
use argh::FromArgs;

use crate::crypto::{self, MnemonicType};
use crate::util::*;

#[derive(FromArgs)]
/// Seed utils
#[argh(subcommand, name = "seed")]
pub struct Cmd {
    #[argh(subcommand)]
    subcommand: SubCmd,
}

impl Cmd {
    pub fn run(self) -> Result<()> {
        match self.subcommand {
            SubCmd::Generate(cmd) => cmd.run(),
            SubCmd::Derive(cmd) => cmd.run(),
            SubCmd::Pubkey(cmd) => cmd.run(),
        }
    }
}

#[derive(FromArgs)]
#[argh(subcommand)]
enum SubCmd {
    Generate(CmdGenerate),
    Derive(CmdDerive),
    Pubkey(CmdPubkey),
}

#[derive(Debug, PartialEq, FromArgs)]
/// Generates new seed
#[argh(subcommand, name = "generate")]
struct CmdGenerate {
    /// mnemonic type
    #[argh(option, long = "type", short = 't', default = "MnemonicType::Bip39")]
    ty: MnemonicType,
}

impl CmdGenerate {
    fn run(self) -> Result<()> {
        let seed = crypto::generate_key(self.ty);
        print_output(seed);
        Ok(())
    }
}

#[derive(Debug, PartialEq, FromArgs)]
/// Derives key from seed
#[argh(subcommand, name = "derive")]
struct CmdDerive {
    /// mnemonic type
    #[argh(option, long = "type", short = 't', default = "MnemonicType::Bip39")]
    ty: MnemonicType,

    /// seed phrase or empty for input from stdin
    #[argh(positional)]
    seed: Option<String>,

    /// derivation path for bip39 mnemonic
    #[argh(option, short = 'p')]
    path: Option<String>,

    /// encode keys in base64 (hex by default)
    #[argh(switch)]
    base64: bool,
}

impl CmdDerive {
    fn run(self) -> Result<()> {
        let seed = parse_optional_input(self.seed, true)?;
        let seed = String::from_utf8(seed)?;

        let path = if let Some(path) = &self.path {
            path.as_str()
        } else {
            crypto::DEFAULT_PATH
        };

        let keys = crypto::derive_from_phrase(seed.trim(), self.ty, path)?;

        print_output(encode_key_pair(keys.secret, keys.public, self.base64));
        Ok(())
    }
}

#[derive(Debug, PartialEq, FromArgs)]
/// Computes public key from secret key
#[argh(subcommand, name = "pubkey")]
struct CmdPubkey {
    /// secret key in hex or empty for input from stdin
    #[argh(positional)]
    secret: Option<String>,

    /// encode keys in base64 (hex by default)
    #[argh(switch)]
    base64: bool,
}

impl CmdPubkey {
    fn run(self) -> Result<()> {
        let secret = parse_optional_input(self.secret, false)?;

        let secret = ed25519_dalek::SecretKey::from_bytes(&secret)?;
        let public = ed25519_dalek::PublicKey::from(&secret);

        print_output(encode_key_pair(secret, public, self.base64));
        Ok(())
    }
}

fn encode_key_pair(
    secret: ed25519_dalek::SecretKey,
    public: ed25519_dalek::PublicKey,
    base64: bool,
) -> serde_json::Value {
    let encode = |bytes: &[u8; 32]| -> String {
        if base64 {
            base64::encode(bytes)
        } else {
            hex::encode(bytes)
        }
    };

    serde_json::json!({
        "secret": encode(secret.as_bytes()),
        "public": encode(public.as_bytes()),
    })
}

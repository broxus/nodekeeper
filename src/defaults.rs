use std::collections::HashMap;

use once_cell::race::OnceBox;

use crate::util::parse_hex_or_base64;

pub const DEFAULT_CURRENCY: &str = "EVER";

pub const DEFAULT_NODE_REPO: &str = "https://github.com/tonlabs/ever-node.git";

pub const DEFAULT_CONTROL_PORT: u16 = 5031;
pub const DEFAULT_LOCAL_ADNL_PORT: u16 = 0;
pub const DEFAULT_ADNL_PORT: u16 = 30100;

const ENV_CURRENCY: &str = "NODEKEEPER_CURRENCY";
const ENV_NODE_REPO: &str = "NODEKEEPER_NODE_REPO";

#[derive(Copy, Clone)]
pub struct Values {
    pub currency: &'static str,
    pub node_repo: &'static str,
}

pub fn currency_from_env() -> Option<&'static str> {
    static ENV_VALUE: OnceBox<Option<String>> = OnceBox::new();
    ENV_VALUE
        .get_or_init(|| Box::new(std::env::var(ENV_CURRENCY).ok()))
        .as_deref()
}

pub fn node_repo_from_env() -> Option<&'static str> {
    static ENV_VALUE: OnceBox<Option<String>> = OnceBox::new();
    ENV_VALUE
        .get_or_init(|| Box::new(std::env::var(ENV_NODE_REPO).ok()))
        .as_deref()
}

macro_rules! decl_known_networks {
    ($ident:ident, { $($file_hash:literal => { currency: $currency:expr, node_repo: $node_repo:expr, }),*$(,)? }) => {
        pub fn $ident(zerostate_file_hash: &[u8; 32]) -> Option<Values> {
            static KNOWN_NETWORKS: OnceBox<HashMap<[u8; 32], Values>> = OnceBox::new();
            KNOWN_NETWORKS.get_or_init(|| Box::new(HashMap::from([
                $((parse_hex_or_base64($file_hash).unwrap().try_into().unwrap(), Values {
                    currency: $currency,
                    node_repo: $node_repo,
                })),*
            ])))
            .get(zerostate_file_hash)
            .copied()
        }
    }
}

decl_known_networks! {
    detect_custom_defaults, {
        "ywj7H75tJ3PgbEeX+UNP3j0iR1x9imIIJJuQgrlCr8s=" => {
            currency: "VENOM",
            node_repo: "https://github.com/tonlabs/ever-node.git -f with_signature_id",
        },
    }
}

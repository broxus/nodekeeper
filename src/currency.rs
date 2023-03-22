use std::collections::HashMap;

use once_cell::race::OnceBox;

use crate::util::parse_hex_or_base64;

pub const DEFAULT: &str = "EVER";

const ENV: &str = "NODEKEEPER_CURRENCY";

pub fn from_env() -> Option<&'static str> {
    static ENV_VALUE: OnceBox<Option<String>> = OnceBox::new();
    ENV_VALUE
        .get_or_init(|| Box::new(std::env::var(ENV).ok()))
        .as_deref()
}

macro_rules! decl_known_networks {
    ($ident:ident, { $($file_hash:literal => $currency:literal),*$(,)? }) => {
        pub fn $ident(zerostate_file_hash: &[u8; 32]) -> Option<&'static str> {
            static KNOWN_NETWORKS: OnceBox<HashMap<[u8; 32], &'static str>> = OnceBox::new();
            KNOWN_NETWORKS.get_or_init(|| Box::new(HashMap::from([
                $((parse_hex_or_base64($file_hash).unwrap().try_into().unwrap(), $currency)),*
            ])))
            .get(zerostate_file_hash)
            .copied()
        }
    }
}

decl_known_networks! {
    detect_custom_currency, {
        "ywj7H75tJ3PgbEeX+UNP3j0iR1x9imIIJJuQgrlCr8s=" => "VENOM",
    }
}

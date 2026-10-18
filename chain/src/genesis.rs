//! Genesis files for a run: a private test genesis built here (funded
//! agent keys, dormant legacy owners, a checkpoint key, an easy target),
//! or the real launch files (`~/.cache/cyber/genesis/{pussy,bostrom}.tade`).

use std::path::{Path, PathBuf};

use soft3::chain::genesis::Genesis;
use soft3::chain::header::encode_bits;
use soft3::chain::params;
use soft3::chain::pool::Pool;
use soft3::chain::sigs::{legacy_owner, neuron_of, token};
use soft3::chain::u256::U256;
use soft3::genesis::file::{Balance, GenesisFile, Link, Params};

pub type Id = [u8; 32];

/// Genesis time of every test genesis (arbitrary, far from the epoch edge).
pub const TEST_GENESIS_TIME: u64 = 1_800_000_000;

/// The test genesis target: 2^248, about 256 hashes a block. With the
/// relaxed limit (2^255) ASERT has 7 halvings of room below and any room
/// above, so a 100× hashrate collapse and spike both stay inside the clamp.
pub const TEST_TARGET_BITS_LOG2: u32 = 248;

pub fn key(seed: u64) -> mudra::SigningKey {
    let mut b = [0u8; 32];
    b[..8].copy_from_slice(&seed.wrapping_add(1).to_le_bytes());
    b[31] = 0x5a;
    mudra::SigningKey::from_bytes((&b).into()).expect("non-zero scalar")
}

pub fn pubkey(k: &mudra::SigningKey) -> [u8; 33] {
    mudra::cosmos::compressed(k.verifying_key())
}

/// An account the run controls.
#[derive(Clone)]
pub struct Account {
    pub key: mudra::SigningKey,
    pub neuron: Id,
}

impl Account {
    pub fn new(seed: u64) -> Account {
        let key = key(seed);
        let neuron = neuron_of(&pubkey(&key));
        Account { key, neuron }
    }
}

/// A dormant legacy owner and the key that may claim it.
#[derive(Clone)]
pub struct Legacy {
    pub key: mudra::SigningKey,
    pub account_id: [u8; 20],
    pub owner: Id,
    pub amount: u64,
}

pub struct TestGenesis {
    pub genesis: Genesis,
    pub accounts: Vec<Account>,
    pub legacy: Vec<Legacy>,
    pub checkpoint_key: mudra::SigningKey,
    pub native: Id,
}

pub struct TestSpec {
    pub accounts: usize,
    pub funds: u64,
    pub legacy: usize,
    pub legacy_funds: u64,
    /// genesis links (weights for φ*): `(author index, p seed, q seed)`
    pub links: Vec<(usize, u64, u64)>,
}

pub fn particle(seed: u64) -> Id {
    let mut b = [0u8; 32];
    b[..8].copy_from_slice(&seed.to_le_bytes());
    *hemera::hash(&b).as_bytes()
}

/// A private test genesis named "simnet" (never a launch chain name, so the
/// shortened epoch clock applies).
pub fn test(spec: &TestSpec) -> TestGenesis {
    let native = token("pussy");
    let accounts: Vec<Account> = (0..spec.accounts as u64).map(Account::new).collect();
    let checkpoint_key = key(1_000_000);
    let legacy: Vec<Legacy> = (0..spec.legacy as u64)
        .map(|i| {
            let k = key(2_000_000 + i);
            let account_id = mudra::cosmos::account_id(&pubkey(&k));
            Legacy { owner: legacy_owner("pussy", &account_id), key: k, account_id, amount: spec.legacy_funds }
        })
        .collect();
    let mut balances: Vec<Balance> = accounts.iter().map(|a| Balance { owner: a.neuron, token: native, amount: spec.funds }).collect();
    balances.extend(legacy.iter().map(|l| Balance { owner: l.owner, token: native, amount: l.amount }));
    balances.sort();
    let s0: u64 = balances.iter().map(|b| b.amount).sum();
    let mut links: Vec<Link> = spec
        .links
        .iter()
        .map(|(a, p, q)| Link { neuron: accounts[*a % accounts.len()].neuron, p: particle(*p), q: particle(*q), height: 1 })
        .collect();
    links.sort_by(|a, b| (a.neuron, a.p, a.q).cmp(&(b.neuron, b.p, b.q)));
    links.dedup_by(|a, b| (a.neuron, a.p, a.q) == (b.neuron, b.p, b.q));
    let file = GenesisFile {
        params: Params {
            version: 1,
            name: "simnet".into(),
            hrp: "pussy".into(),
            denom: "pussy".into(),
            time: TEST_GENESIS_TIME,
            bits: encode_bits(U256::ONE << TEST_TARGET_BITS_LOG2),
            subsidy: params::subsidy(s0),
            checkpoint_key: pubkey(&checkpoint_key),
            checkpoint_sunset: params::CHECKPOINT_SUNSET,
            pool: Pool::genesis(),
        },
        balances,
        links,
    };
    let genesis = Genesis::from_file(&file).expect("test genesis");
    TestGenesis { genesis, accounts, legacy, checkpoint_key, native }
}

/// The launch genesis cache (`~/.cache/cyber/genesis`), or `$CYBER_GENESIS_DIR`.
pub fn launch_dir() -> PathBuf {
    if let Ok(d) = std::env::var("CYBER_GENESIS_DIR") {
        return PathBuf::from(d);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    Path::new(&home).join(".cache/cyber/genesis")
}

pub fn launch(name: &str) -> Result<Genesis, String> {
    let path = launch_dir().join(format!("{name}.tade"));
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    Genesis::decode(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

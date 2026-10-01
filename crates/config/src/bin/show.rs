//! Print `chain_id`, `moniker`, `proxy_app`, and the genesis validator set.

use std::env;
use std::process::ExitCode;

use eld_tendermint_config::{Error, load_home, resolve_home};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("ERROR: {err}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<(), Error> {
    let flag = home_flag()?;
    let home = resolve_home(flag.as_deref())?;
    let config = load_home(&home)?;
    let genesis = config.load_genesis()?;
    println!("chain_id: {}", genesis.chain_id.as_str());
    println!("moniker: {}", config.base.moniker);
    println!("proxy_app: {}", config.base.proxy_app);
    println!("validators:");
    for validator in &genesis.validators {
        println!(
            "  {} power={} name={}",
            hex::encode_upper(&validator.address),
            validator.power,
            validator.name
        );
    }
    Ok(())
}

fn home_flag() -> Result<Option<String>, Error> {
    let mut args = env::args().skip(1);
    let mut home = None;
    while let Some(arg) = args.next() {
        if let Some(value) = arg.strip_prefix("--home=") {
            if value.is_empty() {
                return Err(Error::Usage("missing value for --home".to_owned()));
            }
            home = Some(value.to_owned());
        } else if arg == "--home" {
            let Some(value) = args.next() else {
                return Err(Error::Usage("missing value for --home".to_owned()));
            };
            home = Some(value);
        } else {
            return Err(Error::Usage(format!("unknown argument {arg}")));
        }
    }
    Ok(home)
}

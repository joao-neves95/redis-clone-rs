mod cli;
mod models;
mod node;
mod resp_parser;
mod test_helpers;
mod utils;

use models::db::{AppData, InMemoryDb};

use std::time::Duration;

use anyhow::{Error, Result};

const DEFAULT_LISTENING_PORT: u32 = 6379;
const TCP_READ_TIMEOUT: Duration = Duration::from_millis(1000);
const TCP_READ_TIMEOUT_MAX_RETRIES: u8 = 3;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let cli_flags = cli::parse_cli_args()?;
    let is_replica = cli_flags.replica_of.is_some();

    let app_data = if !is_replica {
        AppData::new_master(cli_flags.port)?
    } else {
        AppData::new_replica(cli_flags.port, cli_flags.replica_of.unwrap().into())
    };

    let mem_db = InMemoryDb::new(app_data)?;

    if is_replica {
        match node::replica_handshake::run(&mem_db).await {
            Err(e) => println!("Error while performing handshake with master: {:?}", e),
            Ok(_) => {}
        };
    }

    node::command_listener::run(&mem_db).await?;

    Ok(())
}

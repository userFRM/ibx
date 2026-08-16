//! The same questions, from inside a runtime.
//!
//! Usage: IB_USERNAME=... IB_PASSWORD=... cargo run --features async --example hello_async

use std::env;

use ibx::types::model::{Contract, Order};
use ibx::{AsyncClient, EClientConfig};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ib = AsyncClient::connect(EClientConfig {
        username: env::var("IB_USERNAME")?,
        password: env::var("IB_PASSWORD")?,
        paper: true,
        ..Default::default()
    })
    .await?;

    let spy = ib.qualify(Contract::stock("SPY")).await?;

    // Two questions at once. Each waits on a thread of its own, so neither
    // holds the runtime while the venue thinks about it.
    let (bars, summary) = tokio::join!(ib.bars(&spy, "2 D", "1 hour"), ib.summary());
    println!("{} bars", bars?.len());
    for value in summary? {
        println!("{:<22} {} {}", value.tag, value.value, value.currency);
    }

    let preview = ib.preview(&spy, &Order::limit("BUY", 1.0, 1.00)).await?;
    println!("that order would cost {}", preview.commission_and_fees);

    ib.disconnect();
    Ok(())
}

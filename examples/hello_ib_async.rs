//! The same session, from inside a runtime.
//!
//! Usage: IB_USERNAME=... IB_PASSWORD=... cargo run --features async --example hello_ib_async

use std::env;
use std::time::Duration;

use ibx::types::model::{Contract, Order};
use ibx::{AsyncIB, EClientConfig};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ib = AsyncIB::connect(EClientConfig {
        username: env::var("IB_USERNAME")?,
        password: env::var("IB_PASSWORD")?,
        paper: true,
        ..Default::default()
    })
    .await?;

    let spy = ib.qualify(Contract::stock("SPY")).await?;

    // Reading what the session holds waits for nothing, so it is not awaited.
    ib.watch(&spy)?;
    ib.wait_on_update(Duration::from_secs(5)).await;
    if let Some(quote) = ib.ticker(&spy) {
        println!("bid {} ask {}", quote.bid, quote.ask);
    }

    let preview = ib.what_if(&spy, &Order::limit("BUY", 1.0, 1.00)).await?;
    println!("that order would cost {}", preview.commission_and_fees);

    for position in ib.positions() {
        println!("{:<8} {:>10}", position.contract.symbol, position.quantity);
    }

    ib.disconnect();
    Ok(())
}
